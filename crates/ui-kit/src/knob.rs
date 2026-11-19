//! A volume knob: a track's volume turned like a knob on a mixing desk,
//! on the one volume scale ([`crate::volume`]).
//!
//! Two looks, one behaviour:
//!
//! - [`KnobLook::Compact`]: a ring that fills up to the value, the value in
//!   the middle, and a small scale around it: −∞, −12, a red 0 and +12,
//!   with coloured dots at −24 and −6 (green) and +6 (orange). Past 0 dB
//!   the ring is orange.
//! - [`KnobLook::Detailed`]: a larger knob with a red pointer on its edge,
//!   a thin ring on its face open at 0 dB, thin lines all the way round
//!   (every other one longer), a blue line at the bottom of the range
//!   (8 o'clock), an orange one at the top (half past 3), 0 dB at 2 o'clock
//!   with its label in red, and labels from −∞ to +12.
//!
//! Turning it: drag in a circle around it (the angle moved is the change,
//! so grabbing it never makes it jump; close to the middle, up and down
//! work instead); the wheel turns it while Ctrl is held, or once the knob
//! has been clicked (until a click elsewhere), half a decibel a notch; Shift
//! makes both finer. A double-click puts it back at 0 dB. While it is
//! dragged or wheeled the value is shown and heard ([`KnobOut::shown_db`]);
//! letting go, or the wheel resting [`WHEEL_SETTLES`], is one change
//! ([`KnobOut::done`]).

use crate::Theme;
use crate::theme::status;
use crate::volume::{self, BOTTOM_DB, TOP_DB};
use egui::{Align2, Color32, FontId, Pos2, Rect, Sense, Stroke, Ui, Vec2, vec2};

/// Orange: louder than as recorded (past 0 dB), and the top of the range.
pub const BOOST: Color32 = Color32::from_rgb(0xE8, 0x91, 0x3A);
/// Blue: the bottom of the range on the detailed knob.
pub const BOTTOM: Color32 = Color32::from_rgb(0x4C, 0x8D, 0xF6);
/// How long the wheel rests before its turns become one change.
pub const WHEEL_SETTLES: f64 = 0.5;
/// One notch of the wheel, in decibels (Shift: [`FINE_NOTCH_DB`]).
pub const NOTCH_DB: f32 = 0.5;
pub const FINE_NOTCH_DB: f32 = 0.1;
/// With Shift held a drag turns it this much slower.
const FINE: f32 = 0.2;

/// Which knob is drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KnobLook {
    Compact,
    Detailed,
}

/// Where the range sits around a knob: angles in degrees clockwise from
/// 12 o'clock for the bottom of the range, 0 dB and the top.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sweep {
    pub bottom: f32,
    pub zero: f32,
    pub top: f32,
}

impl KnobLook {
    /// The space it takes, its scale and labels included.
    pub fn size(self) -> Vec2 {
        match self {
            KnobLook::Compact => vec2(86.0, 62.0),
            KnobLook::Detailed => vec2(150.0, 106.0),
        }
    }

    /// Its middle within a box of [`KnobLook::size`] at `min`.
    fn centre(self, min: Pos2) -> Pos2 {
        match self {
            KnobLook::Compact => min + vec2(43.0, 33.0),
            KnobLook::Detailed => min + vec2(75.0, 66.0),
        }
    }

    /// The knob's own radius (what is grabbed).
    fn radius(self) -> f32 {
        match self {
            KnobLook::Compact => 18.0,
            KnobLook::Detailed => 34.0,
        }
    }

    pub fn sweep(self) -> Sweep {
        match self {
            // 0 dB at half past one, the top at half past four.
            KnobLook::Compact => Sweep {
                bottom: -135.0,
                zero: 45.0,
                top: 135.0,
            },
            // The bottom at 8 o'clock, 0 dB at 2, the top at half past 3.
            KnobLook::Detailed => Sweep {
                bottom: -120.0,
                zero: 60.0,
                top: 105.0,
            },
        }
    }
}

impl Sweep {
    /// The angle for `db`: straight decibels from the bottom to 0 dB, and
    /// from 0 dB to the top.
    pub fn angle(&self, db: f32) -> f32 {
        let db = db.clamp(BOTTOM_DB, TOP_DB);
        if db <= 0.0 {
            self.bottom + (self.zero - self.bottom) * (db - BOTTOM_DB) / -BOTTOM_DB
        } else {
            self.zero + (self.top - self.zero) * db / TOP_DB
        }
    }

    /// The volume at `angle`: the inverse of [`Sweep::angle`].
    pub fn db_at(&self, angle: f32) -> f32 {
        let angle = angle.clamp(self.bottom, self.top);
        if angle <= self.zero {
            BOTTOM_DB + (angle - self.bottom) / (self.zero - self.bottom) * -BOTTOM_DB
        } else {
            (angle - self.zero) / (self.top - self.zero) * TOP_DB
        }
    }
}

/// The point `r` from `centre` at `degrees` clockwise from 12 o'clock.
fn polar(centre: Pos2, r: f32, degrees: f32) -> Pos2 {
    let a = (degrees - 90.0).to_radians();
    centre + vec2(r * a.cos(), r * a.sin())
}

/// The angle of `p` around `centre`, degrees clockwise from 12 o'clock.
fn angle_of(centre: Pos2, p: Pos2) -> f32 {
    let d = p - centre;
    d.y.atan2(d.x).to_degrees() + 90.0
}

/// `to - from` the short way round, in degrees.
fn turned(from: f32, to: f32) -> f32 {
    (to - from + 540.0).rem_euclid(360.0) - 180.0
}

/// What a frame of a knob gives back.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct KnobOut {
    /// The volume to show and hear now: being turned, or as it is.
    pub shown_db: f32,
    /// Turned and let go (or the wheel came to rest, or a double-click):
    /// one change.
    pub done: Option<f32>,
    /// Whether the wheel over it turns it now (Ctrl held, or clicked).
    pub takes_wheel: bool,
}

/// What a knob keeps between frames (in egui's memory, by its id).
#[derive(Debug, Clone, Copy, Default)]
struct Kept {
    /// Being dragged: the volume unrounded (so slow moves add up) and the
    /// pointer's angle last frame.
    drag: Option<(f32, f32)>,
    /// Clicked: the wheel turns it until a click elsewhere.
    focused: bool,
    /// Turned by the wheel and not yet let go: the volume and when the
    /// wheel last moved.
    wheeled: Option<(f32, f64)>,
}

/// A volume knob to draw this frame.
#[derive(Debug, Clone, Copy)]
pub struct Knob<'a> {
    pub look: KnobLook,
    /// The top left of its box ([`KnobLook::size`]).
    pub at: Pos2,
    /// Keeps what it is doing between frames.
    pub id: egui::Id,
    /// The volume it is set to.
    pub db: f32,
    /// Its ring (the compact knob).
    pub colour: Color32,
    /// Said when it is pointed at.
    pub tip: &'a str,
}

/// Draws a volume knob and turns it as the pointer says.
pub fn volume_knob(ui: &mut Ui, theme: &Theme, knob: Knob) -> KnobOut {
    let Knob {
        look,
        at: min,
        id,
        db,
        colour,
        tip,
    } = knob;
    let centre = look.centre(min);
    let radius = look.radius();
    let sweep = look.sweep();
    let grab = Rect::from_center_size(centre, Vec2::splat(radius * 2.0 + 10.0));
    let r = ui
        .interact(grab, id, Sense::click_and_drag())
        .on_hover_cursor(egui::CursorIcon::Grab)
        .on_hover_text(tip);
    let mut kept: Kept = ui.data(|d| d.get_temp(id)).unwrap_or_default();
    let (shift, ctrl, now) = ui.input(|i| (i.modifiers.shift, i.modifiers.command, i.time));
    let mut done = None;

    // A click on it lets the wheel turn it; a click elsewhere stops that.
    if r.clicked() || r.drag_started() {
        kept.focused = true;
    } else if ui.input(|i| i.pointer.any_pressed()) && !r.hovered() {
        kept.focused = false;
    }
    // Dragged around it: the angle moved is the change.
    // From where it was pressed: a drag is noticed a few points later.
    if r.drag_started()
        && let Some(p) = ui
            .input(|i| i.pointer.press_origin())
            .or(r.interact_pointer_pos())
    {
        kept.drag = Some((kept.wheeled.map_or(db, |(w, _)| w), angle_of(centre, p)));
        kept.wheeled = None;
    }
    if let (Some((value, last)), true) = (kept.drag, r.dragged())
        && let Some(p) = r.interact_pointer_pos()
    {
        let scale = if shift { FINE } else { 1.0 };
        let (by_angle, angle) = if (p - centre).length() > radius * 0.35 {
            let angle = angle_of(centre, p);
            (turned(last, angle), angle)
        } else {
            // Close to the middle the angle jumps about: up and down instead.
            (-r.drag_delta().y * 1.5, last)
        };
        let at = sweep.angle(value) + by_angle * scale;
        kept.drag = Some((sweep.db_at(at), angle));
    }
    let settle = |db: f32| match shift {
        true => volume::settled_fine(db),
        false => volume::settled(db),
    };
    if r.drag_stopped()
        && let Some((value, _)) = kept.drag.take()
    {
        let value = settle(value);
        if value != db {
            done = Some(value);
        }
    }
    // The wheel, while it turns it.
    let takes_wheel = r.hovered() && (ctrl || kept.focused);
    if takes_wheel {
        let notches = wheel_notches(ui);
        if notches != 0.0 {
            let step = if shift { FINE_NOTCH_DB } else { NOTCH_DB };
            let from = kept.wheeled.map_or(db, |(w, _)| w);
            let to = (from + notches * step).clamp(BOTTOM_DB, TOP_DB);
            kept.wheeled = Some(((to / step).round() * step, now));
        }
    }
    if let Some((value, at)) = kept.wheeled {
        if now - at >= WHEEL_SETTLES {
            kept.wheeled = None;
            if value != db {
                done = Some(value);
            }
        } else {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(100));
        }
    }
    if r.double_clicked() {
        kept = Kept {
            focused: kept.focused,
            ..Kept::default()
        };
        done = (db != 0.0).then_some(0.0);
    }
    let shown_db = kept
        .drag
        .map(|(v, _)| settle(v))
        .or(kept.wheeled.map(|(v, _)| v))
        .or(done)
        .unwrap_or(db);
    ui.data_mut(|d| d.insert_temp(id, kept));

    let p = ui.painter();
    match look {
        KnobLook::Compact => draw_compact(p, theme, centre, sweep, shown_db, colour, kept.focused),
        KnobLook::Detailed => draw_detailed(p, theme, centre, sweep, shown_db, kept.focused),
    }
    KnobOut {
        shown_db,
        done,
        takes_wheel,
    }
}

/// Whether the knob `id` takes the wheel now: Ctrl is held, or it was
/// clicked last (for a page to keep the wheel off its own scrolling).
pub fn takes_wheel(ui: &Ui, id: egui::Id) -> bool {
    let focused = ui.data(|d| d.get_temp::<Kept>(id)).is_some_and(|k| k.focused);
    focused || ui.input(|i| i.modifiers.command)
}

/// The wheel's turn this frame, in notches (up is louder). egui hands Ctrl
/// with the wheel on as a zoom, so the events themselves are read.
fn wheel_notches(ui: &Ui) -> f32 {
    ui.input(|i| {
        i.events
            .iter()
            .map(|e| match e {
                egui::Event::MouseWheel { unit, delta, .. } => match unit {
                    egui::MouseWheelUnit::Line => delta.y,
                    egui::MouseWheelUnit::Point => delta.y / 50.0,
                    egui::MouseWheelUnit::Page => delta.y * 3.0,
                },
                _ => 0.0,
            })
            .sum()
    })
}

/// An arc of `radius` around `centre` from `from` to `to` degrees.
fn arc(centre: Pos2, radius: f32, from: f32, to: f32) -> Vec<Pos2> {
    let (a, b) = if from <= to { (from, to) } else { (to, from) };
    let steps = ((b - a).abs() / 6.0).ceil().max(1.0) as usize;
    (0..=steps)
        .map(|i| polar(centre, radius, a + (b - a) * i as f32 / steps as f32))
        .collect()
}

fn signed(db: f32) -> String {
    match db {
        d if d <= BOTTOM_DB => "−∞".to_owned(),
        d if d > 0.0 => format!("+{d:.0}"),
        0.0 => "0".to_owned(),
        d => format!("−{:.0}", d.abs()),
    }
}

/// The compact knob: a ring filling up to the value, the value inside, a
/// small scale with coloured dots around it.
fn draw_compact(
    p: &egui::Painter,
    theme: &Theme,
    c: Pos2,
    sweep: Sweep,
    db: f32,
    colour: Color32,
    focused: bool,
) {
    let r = 18.0;
    // Thin ticks all round, and the marks with their labels.
    for i in 0..=16 {
        let a = sweep.bottom + (sweep.top - sweep.bottom) * i as f32 / 16.0;
        p.line_segment(
            [polar(c, 21.5, a), polar(c, 23.0, a)],
            Stroke::new(1.0, theme.line_strong),
        );
    }
    for (mark, size) in [(BOTTOM_DB, 8.0), (-12.0, 8.0), (0.0, 9.5), (TOP_DB, 8.0)] {
        let a = sweep.angle(mark);
        let zero = mark == 0.0;
        let (tick, text) = match zero {
            true => (status::ERROR, status::ERROR),
            false => (theme.muted, theme.faint),
        };
        p.line_segment(
            [polar(c, 21.5, a), polar(c, 25.0, a)],
            Stroke::new(if zero { 1.8 } else { 1.2 }, tick),
        );
        let font = match zero {
            true => FontId::new(size, crate::fonts::semibold()),
            false => FontId::proportional(size),
        };
        p.text(polar(c, 30.5, a), Align2::CENTER_CENTER, signed(mark), font, text);
    }
    for (mark, dot) in [(-24.0, status::GOOD), (-6.0, status::GOOD), (6.0, BOOST)] {
        p.circle_filled(polar(c, 23.5, sweep.angle(mark)), 1.9, dot);
    }
    // The knob: its track, the ring up to the value (orange past 0 dB).
    p.circle_filled(c, r, theme.bg);
    if focused {
        p.circle_stroke(c, r + 0.5, Stroke::new(1.0, theme.text_secondary));
    }
    p.add(egui::Shape::line(
        arc(c, r, sweep.bottom, sweep.top),
        Stroke::new(3.0, theme.line),
    ));
    let ring = if db > 0.0 { BOOST } else { colour };
    p.add(egui::Shape::line(
        arc(c, r, sweep.bottom, sweep.angle(db)),
        Stroke::new(3.0, ring),
    ));
    let value = match db {
        d if d <= BOTTOM_DB => "−60".to_owned(),
        d => volume::number(d, volume::Bottom::Heard),
    };
    p.text(
        c,
        Align2::CENTER_CENTER,
        value,
        FontId::proportional(10.5),
        theme.text,
    );
}

/// The detailed knob: lines all the way round, its range in blue and
/// orange, 0 dB in red, a red pointer on its edge and a ring on its face.
fn draw_detailed(p: &egui::Painter, theme: &Theme, c: Pos2, sweep: Sweep, db: f32, focused: bool) {
    let r = 34.0;
    let (inner, short, long) = (r + 5.0, r + 10.0, r + 15.0);
    let ticks = 30;
    for i in 0..=ticks {
        let a = sweep.bottom + (sweep.top - sweep.bottom) * i as f32 / ticks as f32;
        let out = if i % 2 == 0 { long } else { short };
        p.line_segment(
            [polar(c, inner, a), polar(c, out, a)],
            Stroke::new(1.2, theme.line_strong),
        );
    }
    for (a, colour) in [(sweep.bottom, BOTTOM), (sweep.top, BOOST)] {
        p.line_segment(
            [polar(c, inner - 1.0, a), polar(c, long + 4.0, a)],
            Stroke::new(3.0, colour),
        );
    }
    for mark in [BOTTOM_DB, -24.0, -12.0, -6.0, 0.0, 6.0, TOP_DB] {
        let zero = mark == 0.0;
        let (font, colour) = match zero {
            true => (FontId::new(10.0, crate::fonts::semibold()), status::ERROR),
            false => (FontId::proportional(8.5), theme.faint),
        };
        p.text(
            polar(c, long + 12.0, sweep.angle(mark)),
            Align2::CENTER_CENTER,
            signed(mark),
            font,
            colour,
        );
    }
    // The knob, a little raised: a lighter top left.
    p.circle_filled(c, r, theme.panel);
    p.circle_filled(c + vec2(-3.0, -4.0), r * 0.8, theme.line.gamma_multiply(0.35));
    p.circle_stroke(c, r, Stroke::new(1.5, Color32::from_black_alpha(200)));
    if focused {
        p.circle_stroke(c, r + 2.0, Stroke::new(1.0, theme.text_secondary));
    }
    // The ring on its face, open at 0 dB.
    let gap = 9.0;
    p.add(egui::Shape::line(
        arc(c, r - 6.0, sweep.zero + gap, sweep.zero + 360.0 - gap),
        Stroke::new(1.2, theme.text.gamma_multiply(0.75)),
    ));
    // The red pointer on its edge, pointing out at the value.
    let a = sweep.angle(db);
    let tip = polar(c, r - 1.0, a);
    let side = |d: f32| polar(c, r - 11.0, a + d);
    p.add(egui::Shape::convex_polygon(
        vec![tip, side(7.0), side(-7.0)],
        status::ERROR,
        Stroke::NONE,
    ));
}

/// How a track's volume knob reads when pointed at.
pub fn tip(name: &str, db: f32) -> String {
    format!(
        "{name}: {} · drag around it, or Ctrl+wheel (or click it, then the wheel) · Shift: finer · double-click: 0 dB",
        volume::words(db, volume::Bottom::Heard)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::pos2;

    #[test]
    fn the_range_sits_where_each_knob_says() {
        let d = KnobLook::Detailed.sweep();
        assert_eq!(d.angle(BOTTOM_DB), -120.0, "the bottom at 8 o'clock");
        assert_eq!(d.angle(0.0), 60.0, "0 dB at 2 o'clock");
        assert_eq!(d.angle(TOP_DB), 105.0, "the top at half past 3");
        assert_eq!(d.angle(99.0), 105.0, "held to the range");
        for look in [KnobLook::Compact, KnobLook::Detailed] {
            let s = look.sweep();
            for db in [-60.0, -24.0, -6.0, -0.5, 0.0, 3.0, 12.0] {
                assert!((s.db_at(s.angle(db)) - db).abs() < 1e-3, "{look:?} {db}");
            }
            // Louder turns further round.
            assert!(s.angle(-6.0) < s.angle(0.0) && s.angle(0.0) < s.angle(6.0));
        }
    }

    #[test]
    fn turning_takes_the_short_way_round() {
        assert_eq!(turned(170.0, -170.0), 20.0);
        assert_eq!(turned(-170.0, 170.0), -20.0);
        assert_eq!(turned(10.0, 30.0), 20.0);
        let c = pos2(0.0, 0.0);
        assert!((angle_of(c, pos2(0.0, -10.0))).abs() < 1e-4, "12 o'clock");
        assert!((angle_of(c, pos2(10.0, 0.0)) - 90.0).abs() < 1e-4, "3 o'clock");
        assert!((polar(c, 10.0, 90.0) - pos2(10.0, 0.0)).length() < 1e-4);
    }

    #[test]
    fn labels_say_the_mark() {
        assert_eq!(signed(BOTTOM_DB), "−∞");
        assert_eq!(signed(-12.0), "−12");
        assert_eq!(signed(0.0), "0");
        assert_eq!(signed(12.0), "+12");
        assert!(tip("Game", -6.0).starts_with("Game: −6.0 dB"));
    }

    /// One frame of a knob at (0, 0) for a volume of `db` with `events`.
    fn frame(
        ctx: &egui::Context,
        look: KnobLook,
        db: f32,
        events: Vec<egui::Event>,
        modifiers: egui::Modifiers,
        time: f64,
    ) -> KnobOut {
        let mut out = None;
        let input = egui::RawInput {
            events: std::iter::once(egui::Event::ModifiersChanged(modifiers))
                .chain(events)
                .collect(),
            time: Some(time),
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(400.0, 300.0))),
            ..Default::default()
        };
        let output = ctx.run_ui(input, |ui| {
            let knob = Knob {
                look,
                at: Pos2::ZERO,
                id: egui::Id::unique("k"),
                db,
                colour: Color32::WHITE,
                tip: "tip",
            };
            out = Some(volume_knob(ui, &Theme::CARBON, knob));
        });
        output.drop_without_applying_deltas();
        out.unwrap_or(KnobOut {
            shown_db: f32::NAN,
            done: None,
            takes_wheel: false,
        })
    }

    #[test]
    fn dragging_round_it_turns_it_and_letting_go_is_one_change() {
        let ctx = egui::Context::default();
        let look = KnobLook::Detailed;
        let c = look.centre(Pos2::ZERO);
        let none = egui::Modifiers::NONE;
        let move_to = |p: Pos2| egui::Event::PointerMoved(p);
        let button = |p: Pos2, pressed| egui::Event::PointerButton {
            pos: p,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: none,
        };
        // Grabbed at 12 o'clock, at -12 dB: nothing jumps.
        let start = polar(c, 30.0, 0.0);
        frame(&ctx, look, -12.0, vec![move_to(start)], none, 0.0);
        frame(&ctx, look, -12.0, vec![button(start, true)], none, 0.1);
        let mut t = 0.2;
        // Turned 45° clockwise in small steps: 45 / (180 / 60) = 15 dB louder.
        let mut last = None;
        for i in 1..=9 {
            last = Some(frame(
                &ctx,
                look,
                -12.0,
                vec![move_to(polar(c, 30.0, i as f32 * 5.0))],
                none,
                t,
            ));
            t += 0.05;
        }
        let shown = last.map(|o| o.shown_db).unwrap_or(f32::NAN);
        assert!((shown - 3.0).abs() < 0.6, "{shown}");
        let out = frame(
            &ctx,
            look,
            -12.0,
            vec![button(polar(c, 30.0, 45.0), false)],
            none,
            t,
        );
        let done = out.done.unwrap_or(f32::NAN);
        assert!((done - 3.0).abs() < 0.6, "one change when let go: {done}");
    }

    #[test]
    fn the_wheel_turns_it_with_ctrl_and_settles_into_one_change() {
        let ctx = egui::Context::default();
        let look = KnobLook::Compact;
        let c = look.centre(Pos2::ZERO);
        let wheel = |notches: f32, modifiers| egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Line,
            delta: vec2(0.0, notches),
            phase: egui::TouchPhase::Move,
            source: egui::MouseWheelSource::Unknown,
            modifiers,
        };
        let none = egui::Modifiers::NONE;
        let ctrl = egui::Modifiers::COMMAND;
        frame(&ctx, look, -6.0, vec![egui::Event::PointerMoved(c)], none, 0.0);
        // Without Ctrl (and not clicked) the wheel is the timeline's.
        let out = frame(&ctx, look, -6.0, vec![wheel(2.0, none)], none, 0.1);
        assert!(!out.takes_wheel && out.shown_db == -6.0);
        // With Ctrl: half a decibel a notch, shown at once, one change once it rests.
        let out = frame(&ctx, look, -6.0, vec![wheel(3.0, ctrl)], ctrl, 0.2);
        assert!(out.takes_wheel);
        assert_eq!((out.shown_db, out.done), (-4.5, None));
        let out = frame(&ctx, look, -6.0, Vec::new(), none, 0.3);
        assert_eq!((out.shown_db, out.done), (-4.5, None), "still settling");
        let out = frame(&ctx, look, -6.0, Vec::new(), none, 0.3 + WHEEL_SETTLES);
        assert_eq!(out.done, Some(-4.5));
    }
}
