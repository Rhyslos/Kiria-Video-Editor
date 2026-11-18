//! Drawing the band across the middle of the Audio page: the column of
//! views at its left (Clarity, Spectrum, Loudness, Tracks, Stereo), then
//! the view chosen. The clarity check draws itself (`clarity_draw`); the
//! live pictures are drawn here from `band::Band`.

use super::band::{self, Band, COLUMNS, LOUDNESS_SECONDS, Source, TRACK_SECONDS, View};
use super::clarity::ClarityBand;
use super::clarity_draw::{self, BandAsks};
use super::roles;
use audio::dsp::bands::{COUNT, position};
use egui::{Align2, Color32, FontId, Pos2, Rect, Shape, Stroke, Ui, UiBuilder, pos2, vec2};
use project::Sequence;
use ui_kit::Theme;
use ui_kit::theme::analyser::{FINE, OVER, heat};

/// The views' column at the band's left.
const RAIL: f32 = 92.0;
/// The spectrum's scale, dBFS.
const LOWEST_DB: f32 = -90.0;
const HIGHEST_DB: f32 = 0.0;
/// The loudness scale, LUFS.
const QUIETEST_LUFS: f32 = -50.0;
const LOUDEST_LUFS: f32 = 0.0;
/// The true peak over which a reading is marked, dBTP.
const CEILING: f32 = -1.0;

/// What the band needs to know besides its own state.
pub struct Context<'a> {
    pub seq: &'a Sequence,
    /// What the analyser listens to now, and the chosen track's name.
    pub listening: Option<engine::Listen>,
    pub track_name: Option<&'a str>,
    /// The loudness aimed for, LUFS.
    pub target: Option<f32>,
    /// The roles shown (the page's Show filter).
    pub show: roles::Show,
}

/// Draws the band in `r`: the views' column, then the view chosen.
pub fn band(
    ui: &mut Ui,
    theme: &Theme,
    r: Rect,
    view: &mut View,
    band: &mut Band,
    clarity: &mut ClarityBand,
    cx: &Context<'_>,
) -> BandAsks {
    ui.painter().rect_filled(r, 6.0, theme.panel);
    let rail = Rect::from_min_size(r.min, vec2(RAIL, r.height())).shrink2(vec2(6.0, 8.0));
    let mut col = ui.new_child(
        UiBuilder::new()
            .max_rect(rail)
            .layout(egui::Layout::top_down_justified(egui::Align::Min)),
    );
    col.spacing_mut().item_spacing.y = 1.0;
    for v in View::ALL {
        if col
            .selectable_label(*view == v, v.label())
            .on_hover_text(v.tip())
            .clicked()
        {
            *view = v;
        }
    }
    let rest = Rect::from_min_max(pos2(rail.right() + 2.0, r.top()), r.max);
    ui.painter().line_segment(
        [
            pos2(rest.left(), r.top() + 8.0),
            pos2(rest.left(), r.bottom() - 8.0),
        ],
        Stroke::new(1.0, theme.line),
    );
    if *view == View::Clarity {
        return clarity_draw::band(ui, theme, rest, clarity, cx.seq);
    }
    let inner = rest.shrink2(vec2(12.0, 8.0));
    match *view {
        View::Clarity => {}
        View::Spectrum => spectrum(ui, theme, inner, band, cx),
        View::Loudness => loudness(ui, theme, inner, band, cx),
        View::Tracks => tracks(ui, theme, inner, band, cx),
        View::Stereo => stereo(ui, theme, inner, band, cx),
    }
    BandAsks::default()
}

/// The picture's frame: a darker ground.
fn ground(ui: &Ui, theme: &Theme, r: Rect) -> egui::Painter {
    let p = ui.painter_at(r);
    p.rect_filled(r, 4.0, theme.bg);
    p
}

/// A quiet line in the middle of `r`.
fn note(ui: &Ui, theme: &Theme, r: Rect, text: &str) {
    ui.painter().text(
        r.center(),
        Align2::CENTER_CENTER,
        text,
        FontId::proportional(12.5),
        theme.muted,
    );
}

/// Mix or the chosen track, for Spectrum and Stereo; says what is heard
/// when the EQ tool holds the analyser on its track.
fn source_switch(ui: &mut Ui, theme: &Theme, r: Rect, band: &mut Band, cx: &Context<'_>) {
    let mut row = ui.new_child(
        UiBuilder::new()
            .max_rect(r)
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    let name = cx.track_name.unwrap_or("Track");
    row.selectable_value(&mut band.source, Source::Mix, "Mix")
        .on_hover_text("The mix, as it goes out");
    row.selectable_value(&mut band.source, Source::Track, name)
        .on_hover_text("The track chosen above, after its EQ");
    let held_by_eq = band.source == Source::Mix && matches!(cx.listening, Some(engine::Listen::Track(_)));
    if held_by_eq {
        row.label(
            egui::RichText::new(format!("The EQ is listening to {name}"))
                .color(theme.muted)
                .size(11.5),
        );
    }
}

/// Spectrum: levels by frequency with the highs held, and the spectrogram.
fn spectrum(ui: &mut Ui, theme: &Theme, r: Rect, band: &mut Band, cx: &Context<'_>) {
    let (head, body) = r.split_top_bottom_at_y(r.top() + 22.0);
    source_switch(ui, theme, head, band, cx);
    let body = body.with_min_y(body.top() + 4.0);
    let split = body.left() + body.width() * 0.55;
    let curve = Rect::from_min_max(body.min, pos2(split - 6.0, body.bottom()));
    let gram = Rect::from_min_max(pos2(split + 6.0, body.top()), body.max);
    let p = ground(ui, theme, curve);
    let plot = curve.shrink2(vec2(4.0, 3.0)).with_max_y(curve.bottom() - 12.0);
    let x_of = |hz: f32| plot.left() + plot.width() * position(hz) / COUNT as f32;
    let x_band = |i: usize| plot.left() + plot.width() * (i as f32 + 0.5) / COUNT as f32;
    let y_of = |db: f32| {
        let t = ((db - LOWEST_DB) / (HIGHEST_DB - LOWEST_DB)).clamp(0.0, 1.0);
        plot.bottom() - t * plot.height()
    };
    let small = FontId::proportional(10.0);
    for (hz, text) in [(100.0, "100"), (1_000.0, "1k"), (10_000.0, "10k")] {
        let x = x_of(hz);
        p.line_segment(
            [pos2(x, plot.top()), pos2(x, plot.bottom())],
            Stroke::new(1.0, theme.line),
        );
        p.text(
            pos2(x, curve.bottom() - 1.0),
            Align2::CENTER_BOTTOM,
            text,
            small.clone(),
            theme.muted,
        );
    }
    // A taller band has room for more levels, written at the left.
    let levels: &[f32] = if plot.height() >= 150.0 {
        &[-75.0, -60.0, -45.0, -30.0, -15.0]
    } else {
        &[-60.0, -30.0]
    };
    for &db in levels {
        let y = y_of(db);
        p.line_segment(
            [pos2(plot.left(), y), pos2(plot.right(), y)],
            Stroke::new(1.0, theme.line),
        );
        if plot.height() >= 70.0 {
            p.text(
                pos2(plot.left() + 2.0, y - 1.0),
                Align2::LEFT_BOTTOM,
                format!("{db:.0}").replace('-', "\u{2212}"),
                small.clone(),
                theme.muted,
            );
        }
    }
    match band.levels {
        Some(levels) => {
            let line: Vec<Pos2> = (0..COUNT).map(|i| pos2(x_band(i), y_of(levels[i]))).collect();
            let mut area = line.clone();
            area.push(pos2(plot.right(), plot.bottom()));
            area.push(pos2(plot.left(), plot.bottom()));
            // Filled as thin columns: a filled outline that isn't convex
            // would be drawn wrong.
            for pair in line.windows(2) {
                p.add(Shape::convex_polygon(
                    vec![
                        pair[0],
                        pair[1],
                        pos2(pair[1].x, plot.bottom()),
                        pos2(pair[0].x, plot.bottom()),
                    ],
                    theme.accent.gamma_multiply(0.18),
                    Stroke::NONE,
                ));
            }
            p.add(Shape::line(line, Stroke::new(1.5, theme.accent)));
            if let Some(held) = band.held {
                let held: Vec<Pos2> = (0..COUNT).map(|i| pos2(x_band(i), y_of(held[i]))).collect();
                p.add(Shape::line(
                    held,
                    Stroke::new(1.0, theme.muted.gamma_multiply(0.7)),
                ));
            }
        }
        None => note(ui, theme, curve, "Play to see the sound by frequency"),
    }
    spectrogram(ui, theme, gram, band);
}

/// The last few seconds, time across (newest at the right), frequency up.
fn spectrogram(ui: &mut Ui, theme: &Theme, r: Rect, band: &mut Band) {
    let p = ground(ui, theme, r);
    if band.columns.is_empty() {
        note(ui, theme, r, "And the last few seconds here");
        return;
    }
    let id = egui::Id::unique("audio band spectrogram");
    let mut texture: Option<egui::TextureHandle> = ui.data(|d| d.get_temp(id));
    if band.columns_changed || texture.is_none() {
        let mut image = egui::ColorImage::filled([COLUMNS, COUNT], heat(0.0));
        let start = COLUMNS - band.columns.len();
        for (c, levels) in band.columns.iter().enumerate() {
            for (b, db) in levels.iter().enumerate() {
                let t = (db - LOWEST_DB) / (HIGHEST_DB - LOWEST_DB);
                image[(start + c, COUNT - 1 - b)] = heat(t);
            }
        }
        let options = egui::TextureOptions::LINEAR;
        match &mut texture {
            Some(t) => t.set(image, options),
            None => texture = Some(ui.ctx().load_texture("spectrogram", image, options)),
        }
        band.columns_changed = false;
        if let Some(t) = &texture {
            ui.data_mut(|d| d.insert_temp(id, t.clone()));
        }
    }
    if let Some(t) = texture {
        let inside = r.shrink(1.0);
        p.image(
            t.id(),
            inside,
            Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
            Color32::WHITE,
        );
    }
}

/// Loudness: momentary and short-term over the last 30 seconds against
/// the aim, true peaks over the ceiling marked along the top.
fn loudness(ui: &mut Ui, theme: &Theme, r: Rect, band: &Band, cx: &Context<'_>) {
    let numbers_w = 150.0;
    let graph = r.with_max_x(r.right() - numbers_w - 10.0);
    let numbers = r.with_min_x(graph.right() + 10.0);
    let p = ground(ui, theme, graph);
    let plot = graph.shrink2(vec2(30.0, 6.0)).with_max_x(graph.right() - 6.0);
    let y_of = |lufs: f32| {
        let t = ((lufs - QUIETEST_LUFS) / (LOUDEST_LUFS - QUIETEST_LUFS)).clamp(0.0, 1.0);
        plot.bottom() - t * plot.height()
    };
    let x_of = |at: f64| {
        let t = 1.0 - ((band.now - at) / LOUDNESS_SECONDS).clamp(0.0, 1.0) as f32;
        plot.left() + t * plot.width()
    };
    let small = FontId::proportional(10.0);
    for lufs in [-40.0, -30.0, -20.0, -10.0] {
        let y = y_of(lufs);
        p.line_segment(
            [pos2(plot.left(), y), pos2(plot.right(), y)],
            Stroke::new(1.0, theme.line),
        );
        p.text(
            pos2(graph.left() + 4.0, y),
            Align2::LEFT_CENTER,
            format!("{lufs:.0}").replace('-', "\u{2212}"),
            small.clone(),
            theme.muted,
        );
    }
    if let Some(target) = cx.target {
        let y = y_of(target);
        p.add(Shape::dashed_line(
            &[pos2(plot.left(), y), pos2(plot.right(), y)],
            Stroke::new(1.0, FINE),
            5.0,
            4.0,
        ));
    }
    if band.loudness.is_empty() {
        note(ui, theme, graph, "Play to see the mix's loudness over time");
    }
    let line = |pick: fn(&band::Reading) -> f32| -> Vec<Vec<Pos2>> {
        // Silence breaks the line rather than dropping it to the floor.
        let mut runs = vec![Vec::new()];
        for reading in &band.loudness {
            let v = pick(reading);
            match runs.last_mut() {
                Some(run) if v.is_finite() => run.push(pos2(x_of(reading.at), y_of(v))),
                _ => runs.push(Vec::new()),
            }
        }
        runs.into_iter().filter(|r| r.len() > 1).collect()
    };
    for run in line(|r| r.momentary) {
        p.add(Shape::line(
            run,
            Stroke::new(1.0, theme.muted.gamma_multiply(0.7)),
        ));
    }
    for run in line(|r| r.short_term) {
        p.add(Shape::line(run, Stroke::new(2.0, theme.accent)));
    }
    for reading in band.loudness.iter().filter(|r| r.true_peak > CEILING) {
        let x = x_of(reading.at);
        p.line_segment(
            [pos2(x, plot.top()), pos2(x, plot.top() + 6.0)],
            Stroke::new(2.0, OVER),
        );
    }
    // Beside it: short-term now, large, and the aim.
    let mut col = ui.new_child(
        UiBuilder::new()
            .max_rect(numbers)
            .layout(egui::Layout::top_down(egui::Align::Min)),
    );
    col.spacing_mut().item_spacing.y = 2.0;
    let now = band.loudness.back();
    let text = |v: Option<f32>| match v.filter(|v| v.is_finite()) {
        Some(v) => format!("{v:.1} LUFS").replace('-', "\u{2212}"),
        None => "\u{2014}".into(),
    };
    col.label(egui::RichText::new("Short-term").color(theme.muted).size(11.5));
    col.label(
        egui::RichText::new(text(now.map(|r| r.short_term)))
            .color(theme.text)
            .size(20.0)
            .strong(),
    );
    col.label(
        egui::RichText::new(format!("Momentary {}", text(now.map(|r| r.momentary))))
            .color(theme.text_secondary)
            .size(11.5),
    );
    let aim = match cx.target {
        Some(t) => format!("Aim {}", text(Some(t))),
        None => "No aim set".into(),
    };
    col.label(egui::RichText::new(aim).color(FINE).size(11.5));
    let overs = band.loudness.iter().filter(|r| r.true_peak > CEILING).count();
    if overs > 0 {
        col.label(
            egui::RichText::new(
                format!("Peaks over {CEILING:.0} dBTP: marked in red").replace('-', "\u{2212}"),
            )
            .color(OVER)
            .size(11.0),
        );
    }
}

/// Tracks: a lane each, how loud it was over the last 15 seconds.
fn tracks(ui: &mut Ui, theme: &Theme, r: Rect, band: &Band, cx: &Context<'_>) {
    let shown: Vec<&project::Track> = cx
        .seq
        .audio_tracks
        .iter()
        .filter(|t| cx.show.shows(t.sound.role))
        .collect();
    if shown.is_empty() {
        note(ui, theme, r, "No audio tracks to show");
        return;
    }
    let names_w = 110.0;
    let lanes = r.with_min_x(r.left() + names_w);
    let p = ground(ui, theme, lanes);
    let most = ((r.height() / 9.0).floor() as usize).max(1);
    let fits = shown.len().min(most);
    let more = shown.len() - fits;
    let lane_h = r.height() / fits as f32;
    let x_of = |at: f64| {
        let t = 1.0 - ((band.now - at) / TRACK_SECONDS).clamp(0.0, 1.0) as f32;
        lanes.left() + t * lanes.width()
    };
    let small = FontId::proportional(if lane_h < 14.0 { 9.5 } else { 11.0 });
    let step = lanes.width() / (TRACK_SECONDS / band::EVERY) as f32;
    for (i, track) in shown.iter().take(fits).enumerate() {
        let top = r.top() + i as f32 * lane_h;
        let lane = Rect::from_min_size(pos2(lanes.left(), top), vec2(lanes.width(), lane_h));
        let colour = roles::colour(track.sound.role);
        let mut name = track.name.clone();
        if i + 1 == fits && more > 0 {
            name = format!("{name} (and {more} more)");
        }
        ui.painter()
            .with_clip_rect(r.with_max_x(lanes.left() - 4.0))
            .text(
                pos2(r.left(), lane.center().y),
                Align2::LEFT_CENTER,
                name,
                small.clone(),
                theme.text_secondary,
            );
        if i > 0 {
            p.line_segment([lane.left_top(), lane.right_top()], Stroke::new(1.0, theme.panel));
        }
        let Some((_, readings)) = band.tracks.get(&track.id) else {
            continue;
        };
        for reading in readings {
            if !reading.momentary.is_finite() {
                continue;
            }
            let t = ((reading.momentary - QUIETEST_LUFS) / (LOUDEST_LUFS - QUIETEST_LUFS)).clamp(0.0, 1.0);
            if t <= 0.0 {
                continue;
            }
            let x = x_of(reading.at);
            let h = (lane_h - 2.0) * t;
            let bar = Rect::from_min_max(
                pos2(x - step, lane.bottom() - 1.0 - h),
                pos2(x, lane.bottom() - 1.0),
            );
            // Turned down by ducking: drawn fainter, so the voice's
            // moments show in what plays under it.
            let alpha = if reading.ducked > 1.0 { 0.45 } else { 0.9 };
            p.rect_filled(bar, 0.0, colour.gamma_multiply(alpha));
        }
    }
    if band.tracks.is_empty() {
        note(ui, theme, lanes, "Play to see who is heard when");
    }
}

/// Stereo: left against right as a picture, how alike the channels are
/// and which side is louder, in words.
fn stereo(ui: &mut Ui, theme: &Theme, r: Rect, band: &mut Band, cx: &Context<'_>) {
    // The square grows with the band, leaving room beside it for the meters.
    let side = r.height().min(r.width() - 360.0).max(r.height().min(120.0));
    let square = Rect::from_min_size(r.min, vec2(side, side));
    let p = ground(ui, theme, square);
    let c = square.center();
    let half = side / 2.0 - 4.0;
    for (a, b) in [
        (pos2(c.x, square.top() + 4.0), pos2(c.x, square.bottom() - 4.0)),
        (pos2(square.left() + 4.0, c.y), pos2(square.right() - 4.0, c.y)),
    ] {
        p.line_segment([a, b], Stroke::new(1.0, theme.line));
    }
    let small = FontId::proportional(9.5);
    p.text(
        pos2(c.x - half * 0.7, square.top() + 6.0),
        Align2::CENTER_TOP,
        "L",
        small.clone(),
        theme.muted,
    );
    p.text(
        pos2(c.x + half * 0.7, square.top() + 6.0),
        Align2::CENTER_TOP,
        "R",
        small.clone(),
        theme.muted,
    );
    // Scaled so the loudest point of the newest picture reaches the edge:
    // the shape matters, not how loud it is.
    let loudest = band
        .stereo
        .back()
        .map(|s| {
            s.points
                .iter()
                .map(|(l, r)| l.abs().max(r.abs()))
                .fold(0.0f32, f32::max)
        })
        .unwrap_or(0.0);
    let scale = if loudest > 1e-4 { 0.7 / loudest } else { 0.0 };
    let pictures = band.stereo.len();
    for (age, picture) in band.stereo.iter().enumerate() {
        let alpha = 0.25 + 0.6 * (age + 1) as f32 / pictures as f32;
        for (l, r) in &picture.points {
            let (x, y) = band::turned(*l, *r, scale);
            let at = pos2(c.x + x.clamp(-1.0, 1.0) * half, c.y - y.clamp(-1.0, 1.0) * half);
            p.circle_filled(at, 1.1, theme.accent.gamma_multiply(alpha));
        }
    }
    let rest = Rect::from_min_max(pos2(square.right() + 14.0, r.top()), r.max);
    let (head, body) = rest.split_top_bottom_at_y(rest.top() + 22.0);
    source_switch(ui, theme, head, band, cx);
    let meter_w = (body.width() * 0.5).clamp(140.0, 320.0);
    let mut y = body.top() + 6.0;
    for (label, value, left, right, warn) in [
        (
            "Phase",
            band.correlation,
            "\u{2212}1",
            "+1",
            band.correlation.is_some_and(|c| c < -0.1),
        ),
        (
            "Balance",
            band.correlation.map(|_| band.balance),
            "L",
            "R",
            band.balance.abs() > 0.3,
        ),
    ] {
        let line = Rect::from_min_size(pos2(body.left() + 60.0, y), vec2(meter_w, 10.0));
        let painter = ui.painter();
        painter.text(
            pos2(body.left(), line.center().y),
            Align2::LEFT_CENTER,
            label,
            FontId::proportional(11.5),
            theme.text_secondary,
        );
        painter.rect_filled(line, 3.0, theme.bg);
        painter.line_segment(
            [
                pos2(line.center().x, line.top() - 2.0),
                pos2(line.center().x, line.bottom() + 2.0),
            ],
            Stroke::new(1.0, theme.line_strong),
        );
        for (text, x, align) in [
            (left, line.left() - 4.0, Align2::RIGHT_CENTER),
            (right, line.right() + 4.0, Align2::LEFT_CENTER),
        ] {
            painter.text(pos2(x, line.center().y), align, text, small.clone(), theme.muted);
        }
        if let Some(v) = value {
            let x = line.left() + line.width() * (v.clamp(-1.0, 1.0) + 1.0) / 2.0;
            let colour = if warn { OVER } else { theme.accent };
            painter.rect_filled(
                Rect::from_center_size(pos2(x, line.center().y), vec2(4.0, 14.0)),
                1.0,
                colour,
            );
        }
        y += 22.0;
    }
    let words = band::stereo_words(band.correlation, band.balance);
    let warn = band.correlation.is_some_and(|c| c < -0.1);
    ui.painter().text(
        pos2(body.left(), y + 4.0),
        Align2::LEFT_TOP,
        words,
        FontId::proportional(12.5),
        if warn { OVER } else { theme.text },
    );
    if band.stereo.is_empty() {
        note(ui, theme, square, "Play");
    }
}
