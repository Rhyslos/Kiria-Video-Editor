//! The clarity check's band across the middle of the page: what is
//! checked (around the playhead, or the region) with the button that
//! switches, how far a check has come, the findings worst first (each its
//! sentence, where it is and its fix buttons), and a small picture of the
//! tracks' levels across the bands with the shared bands marked.

use super::clarity::{ClarityBand, Scope, colour};
use super::roles;
use egui::{Align, Align2, FontId, Layout, Rect, RichText, Sense, Stroke, Ui, UiBuilder, pos2, vec2};
use engine::clarity::bands::{CENTRES, COUNT};
use engine::clarity::{Finding, Fix, Problem, Remedy, Report};
use project::{Sequence, Time, TrackId, TrackRole};
use ui_kit::Theme;

/// The header column's width.
const HEADER: f32 = 178.0;
/// The picture's width, when there is room for it.
const PICTURE: f32 = 250.0;
/// Below this width the picture is left out.
const ROOM_FOR_PICTURE: f32 = 820.0;
/// A finding's row.
const ROW: f32 = 46.0;
/// The picture's level scale, dBFS.
const LOWEST_DB: f32 = -80.0;
const HIGHEST_DB: f32 = -6.0;

/// What the band asks of the window.
#[derive(Debug, Default)]
pub struct BandAsks {
    pub fix: Option<Fix>,
    pub scope: Option<Scope>,
    pub seek: Option<Time>,
}

/// A moment as "0:04.2".
fn at(t: Time) -> String {
    let s = t.as_secs_f64().max(0.0);
    format!("{}:{:04.1}", (s / 60.0).floor() as u64, s % 60.0)
}

/// What a fix's button says; the whole label is its tip.
fn button_text(fix: &Fix) -> &'static str {
    match fix.remedy {
        Remedy::LowerUnderDialogue { .. } => "Lower under dialogue",
        Remedy::CutLowMiddle => "Cut around 300 Hz",
        Remedy::MakeRoomForVoice => "Make room for the voice",
    }
}

/// A decibel figure as a button writes it: "\u{2212}3.5 dB".
fn db(mdb: i32) -> String {
    format!("{:.1} dB", f64::from(mdb) / 1_000.0).replace('-', "\u{2212}")
}

/// A button's text, with its track's name when the finding offers the same
/// fix on several tracks ("Cut Music around 300 Hz"), the name the track
/// has everywhere else on the page. A fix already made says so and offers
/// to go further ("Cut applied, \u{2212}3.5 dB · Cut deeper").
fn button_for(fix: &Fix, fixes: &[Fix], seq: &Sequence) -> String {
    let alike = fixes
        .iter()
        .filter(|f| button_text(f) == button_text(fix))
        .count();
    let name = (alike >= 2)
        .then(|| seq.audio_tracks.iter().find(|t| t.id == fix.track))
        .flatten()
        .map(|t| t.name.as_str());
    match (fix.remedy, fix.applied_mdb, name) {
        (Remedy::LowerUnderDialogue { .. }, Some(d), None) => {
            format!("Lowered, {} · Lower further", db(d))
        }
        (Remedy::LowerUnderDialogue { .. }, Some(d), Some(n)) => {
            format!("{n} lowered, {} · Lower further", db(d))
        }
        (Remedy::CutLowMiddle | Remedy::MakeRoomForVoice, Some(d), None) => {
            format!("Cut applied, {} · Cut deeper", db(d))
        }
        (Remedy::CutLowMiddle | Remedy::MakeRoomForVoice, Some(d), Some(n)) => {
            format!("{n} cut {} · Cut deeper", db(d))
        }
        (_, None, None) => button_text(fix).to_owned(),
        (Remedy::LowerUnderDialogue { .. }, None, Some(n)) => format!("Lower {n} under dialogue"),
        (Remedy::CutLowMiddle, None, Some(n)) => format!("Cut {n} around 300 Hz"),
        (Remedy::MakeRoomForVoice, None, Some(n)) => format!("Make room in {n}"),
    }
}

/// Draws the band in `r`.
pub fn band(ui: &mut Ui, theme: &Theme, r: Rect, band: &mut ClarityBand, seq: &Sequence) -> BandAsks {
    let mut asks = BandAsks::default();
    ui.painter().rect_filled(r, 6.0, theme.panel);
    let inner = r.shrink2(vec2(12.0, 8.0));
    let picture = (r.width() >= ROOM_FOR_PICTURE)
        .then(|| Rect::from_min_max(pos2(inner.right() - PICTURE, inner.top()), inner.max));
    let head = Rect::from_min_size(inner.min, vec2(HEADER, inner.height()));
    let list = Rect::from_min_max(
        pos2(head.right() + 12.0, inner.top()),
        pos2(picture.map_or(inner.right(), |p| p.left() - 14.0), inner.bottom()),
    );
    header(ui, theme, head, band, seq, &mut asks);
    let findings = band.findings(seq.id).to_vec();
    if band.chosen >= findings.len() {
        band.chosen = 0;
    }
    let current = band.report().is_some_and(|(_, current)| current);
    let mut child = ui.new_child(
        UiBuilder::new()
            .max_rect(list)
            .layout(Layout::top_down(Align::Min)),
    );
    if !current {
        child.set_opacity(0.5);
    }
    findings_list(&mut child, theme, &findings, band, seq, &mut asks);
    if let Some(p) = picture {
        let report = band.report().map(|(r, _)| r.clone());
        let mut painter = ui.painter_at(p);
        if !current {
            painter.set_opacity(0.5);
        }
        draw_picture(
            &painter,
            theme,
            p,
            report.as_ref(),
            findings.get(band.chosen),
            seq,
        );
    }
    asks
}

/// The header column: the band's name, what it checks, the switch, progress.
fn header(ui: &mut Ui, theme: &Theme, r: Rect, band: &ClarityBand, seq: &Sequence, asks: &mut BandAsks) {
    let mut ui = ui.new_child(UiBuilder::new().max_rect(r).layout(Layout::top_down(Align::Min)));
    ui.spacing_mut().item_spacing.y = 4.0;
    ui.label(RichText::new("Clarity check").color(theme.text).strong());
    let what = match band.scope {
        Scope::Playhead => "Around the playhead",
        Scope::Region => "Over the region",
    };
    ui.label(RichText::new(what).color(theme.text_secondary).size(12.0));
    match band.scope {
        Scope::Playhead => {
            let has_region = seq.region.is_some();
            let button = ui.add_enabled(has_region, egui::Button::new("Check the region").small());
            let button = if has_region {
                button.on_hover_text("Check the whole region instead of a moment around the playhead.")
            } else {
                button.on_disabled_hover_text("Mark a region first: in and out on the timeline.")
            };
            if button.clicked() {
                asks.scope = Some(Scope::Region);
            }
        }
        Scope::Region => {
            if ui
                .add(egui::Button::new("Back to the playhead").small())
                .on_hover_text("Check a moment around the playhead again, whenever it rests.")
                .clicked()
            {
                asks.scope = Some(Scope::Playhead);
            }
        }
    }
    if let Some(done) = band.progress() {
        ui.add(
            egui::ProgressBar::new(done)
                .desired_width(r.width() - 8.0)
                .desired_height(6.0),
        );
        ui.label(
            RichText::new(format!("Checking… {:.0} %", done * 100.0))
                .color(theme.muted)
                .size(11.5),
        );
    }
}

/// The findings, worst first, or a quiet line when there are none.
fn findings_list(
    ui: &mut Ui,
    theme: &Theme,
    findings: &[Finding],
    band: &mut ClarityBand,
    seq: &Sequence,
    asks: &mut BandAsks,
) {
    if findings.is_empty() {
        let text = match band.report() {
            None if band.progress().is_some() => "Listening…",
            None => "Checks where tracks share frequencies once the playhead rests.",
            Some((r, _)) if r.tracks.is_empty() => "No track is heard here.",
            Some(_) => "Nothing covers the voice here.",
        };
        ui.add_space(ui.available_height() * 0.5 - 9.0);
        ui.label(RichText::new(text).color(theme.text_secondary));
        return;
    }
    egui::ScrollArea::vertical()
        .id_salt("clarity-findings")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            for (i, f) in findings.iter().enumerate() {
                finding_row(ui, theme, i, f, band, seq, asks);
            }
        });
}

fn finding_row(
    ui: &mut Ui,
    theme: &Theme,
    i: usize,
    f: &Finding,
    band: &mut ClarityBand,
    seq: &Sequence,
    asks: &mut BandAsks,
) {
    let width = ui.available_width();
    let (row, response) = ui.allocate_exact_size(vec2(width, ROW), Sense::click());
    if band.chosen == i {
        ui.painter().rect_filled(row, 4.0, theme.raised);
    } else if response.hovered() {
        ui.painter()
            .rect_filled(row, 4.0, theme.raised.gamma_multiply(0.5));
    }
    if response.clicked() {
        band.chosen = i;
    }
    let inner = row.shrink2(vec2(8.0, 3.0));
    let (top, bottom) = inner.split_top_bottom_at_fraction(0.5);
    // First line: what is wrong, in plain words.
    let mut first = ui.new_child(
        UiBuilder::new()
            .max_rect(top)
            .layout(Layout::left_to_right(Align::Center)),
    );
    let (dot, _) = first.allocate_exact_size(vec2(10.0, 10.0), Sense::hover());
    first
        .painter()
        .circle_filled(dot.center(), 4.0, colour(f.problem));
    let tracks: Vec<String> = f
        .tracks
        .iter()
        .filter_map(|t| seq.audio_tracks.iter().find(|a| a.id == *t))
        .map(|t| t.name.clone())
        .collect();
    let label = first.add(
        egui::Label::new(RichText::new(&f.words).color(theme.text))
            .truncate()
            .sense(Sense::click()),
    );
    if label
        .on_hover_text(format!(
            "{}\nHow bad: {:.0} %. Tracks: {}.",
            f.words,
            f.severity * 100.0,
            tracks.join(", ")
        ))
        .clicked()
    {
        band.chosen = i;
    }
    // Second line: where it is, and the fixes.
    let mut second = ui.new_child(
        UiBuilder::new()
            .max_rect(bottom.with_min_x(bottom.left() + 14.0))
            .layout(Layout::left_to_right(Align::Center)),
    );
    let when = format!("{}–{}", at(f.range.0), at(f.range.1));
    let was = band.was(f);
    if second
        .add(
            egui::Label::new(
                RichText::new(when)
                    .monospace()
                    .size(11.5)
                    .color(theme.text_secondary),
            )
            .sense(Sense::click()),
        )
        .on_hover_text("Go there")
        .clicked()
    {
        asks.seek = Some(f.range.0);
    }
    if let Some(was) = was {
        second.add_space(6.0);
        second
            .label(
                RichText::new(format!(
                    "How bad: {:.0} % \u{2192} {:.0} %",
                    was * 100.0,
                    f.severity * 100.0
                ))
                .size(11.5)
                .color(if f.severity < was {
                    ui_kit::status::GOOD
                } else {
                    theme.text_secondary
                }),
            )
            .on_hover_text("How bad it was before the last fix, and how bad it is now.");
    }
    second.add_space(6.0);
    for fix in &f.fixes {
        let tip = format!("{}. One step: undo takes it back.", fix.label);
        if second
            .small_button(button_for(fix, &f.fixes, seq))
            .on_hover_text(tip)
            .clicked()
        {
            band.fixed(f);
            asks.fix = Some(fix.clone());
        }
    }
}

/// Where each involved track's sound is across the bands, the bands the
/// finding is about shaded.
fn draw_picture(
    p: &egui::Painter,
    theme: &Theme,
    r: Rect,
    report: Option<&Report>,
    finding: Option<&Finding>,
    seq: &Sequence,
) {
    p.rect_filled(r, 4.0, theme.bg);
    let plot = Rect::from_min_max(
        pos2(r.left() + 6.0, r.top() + 16.0),
        pos2(r.right() - 6.0, r.bottom() - 14.0),
    );
    let x_of = |band: f32| plot.left() + plot.width() * (band + 0.5) / COUNT as f32;
    let y_of = |db: f32| {
        let t = ((db - LOWEST_DB) / (HIGHEST_DB - LOWEST_DB)).clamp(0.0, 1.0);
        plot.bottom() - t * plot.height()
    };
    let small = FontId::proportional(10.5);
    for (hz, text) in [
        (200.0, "200"),
        (500.0, "500"),
        (1_000.0, "1k"),
        (2_500.0, "2.5k"),
        (6_300.0, "6.3k"),
    ] {
        if let Some(i) = CENTRES.iter().position(|c| *c == hz) {
            let x = x_of(i as f32);
            p.line_segment(
                [pos2(x, plot.top()), pos2(x, plot.bottom())],
                Stroke::new(1.0, theme.line),
            );
            p.text(
                pos2(x, r.bottom() - 2.0),
                Align2::CENTER_BOTTOM,
                text,
                small.clone(),
                theme.muted,
            );
        }
    }
    let Some(report) = report else { return };
    let role_of = |id: TrackId| seq.audio_tracks.iter().find(|t| t.id == id).map(|t| t.sound.role);
    // The tracks drawn: the finding's (with the dialogue it covers), or
    // the loudest few when there is nothing to show.
    let mut drawn: Vec<TrackId> = match finding {
        Some(f) => {
            let mut ids = f.tracks.clone();
            if f.problem == Problem::CoversVoice {
                ids.extend(
                    report
                        .tracks
                        .iter()
                        .filter(|t| role_of(t.track) == Some(TrackRole::Dialogue))
                        .map(|t| t.track),
                );
            }
            ids
        }
        None => {
            let mut loudest: Vec<(TrackId, f32)> = report
                .tracks
                .iter()
                .map(|t| (t.track, t.levels_db.iter().copied().fold(f32::MIN, f32::max)))
                .collect();
            loudest.sort_by(|a, b| b.1.total_cmp(&a.1));
            loudest.into_iter().map(|(t, _)| t).collect()
        }
    };
    drawn.truncate(4);
    if let Some(f) = finding {
        let first = CENTRES.iter().position(|c| *c as u32 == f.band_hz.0).unwrap_or(0);
        let last = CENTRES
            .iter()
            .position(|c| *c as u32 == f.band_hz.1)
            .unwrap_or(COUNT - 1);
        let shade = Rect::from_min_max(
            pos2(x_of(first as f32 - 0.5), plot.top()),
            pos2(x_of(last as f32 + 0.5), plot.bottom()),
        );
        p.rect_filled(shade, 2.0, colour(f.problem).gamma_multiply(0.22));
    }
    let mut legend_x = r.left() + 6.0;
    for id in &drawn {
        let Some(levels) = report.tracks.iter().find(|t| t.track == *id) else {
            continue;
        };
        let c = roles::colour(role_of(*id).unwrap_or_default());
        let points: Vec<egui::Pos2> = levels
            .levels_db
            .iter()
            .enumerate()
            .map(|(i, db)| pos2(x_of(i as f32), y_of(*db)))
            .collect();
        p.add(egui::Shape::line(points, Stroke::new(1.6, c)));
        let name = seq
            .audio_tracks
            .iter()
            .find(|t| t.id == *id)
            .map_or_else(String::new, |t| t.name.clone());
        let galley = p.layout_no_wrap(name, small.clone(), c);
        if legend_x + galley.size().x <= r.right() - 4.0 {
            let w = galley.size().x;
            p.galley(pos2(legend_x, r.top() + 2.0), galley, c);
            legend_x += w + 10.0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Buttons name tracks by their names, as the rest of the page does,
    /// and a fix already made says so.
    #[test]
    fn buttons_say_track_names_and_what_is_already_done() {
        let mut p = project::Project::new_automatic("T");
        let seq_id = p.active_sequence;
        let music = p.new_track(project::TrackKind::Audio, "Music");
        let (first, music_id) = (p.sequences[0].audio_tracks[0].id, music.id);
        p.sequences[0].audio_tracks.push_back(music);
        let seq = p.sequence(seq_id).unwrap().clone();
        let fix = |track, applied_mdb| Fix {
            sequence: seq_id,
            track,
            remedy: Remedy::CutLowMiddle,
            label: String::new(),
            applied_mdb,
        };
        let two = [fix(first, None), fix(music_id, Some(-3_500))];
        assert_eq!(button_for(&two[0], &two, &seq), "Cut A1 around 300 Hz");
        assert_eq!(
            button_for(&two[1], &two, &seq),
            "Music cut \u{2212}3.5 dB · Cut deeper"
        );
        let one = [fix(music_id, Some(-3_500))];
        assert_eq!(
            button_for(&one[0], &one, &seq),
            "Cut applied, \u{2212}3.5 dB · Cut deeper"
        );
    }

    #[test]
    fn moments_are_minutes_and_seconds_to_a_tenth() {
        assert_eq!(at(Time::seconds(4)), "0:04.0");
        assert_eq!(at(Time::seconds(75)), "1:15.0");
    }
}
