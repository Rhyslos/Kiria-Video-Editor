//! The level line: a horizontal line across each clip of an audio track
//! whose level button is on, at the height of the clip's volume. Dragging
//! it up or down changes that clip's volume; a double-click puts it back
//! to 0 dB.
//!
//! Where the line sits, from the bottom of the clip (0) to its top (1):
//! 0 dB in the middle, straight decibels from there up to +12 dB at the
//! top, and a curve below that leaves room for the useful cuts (−6 dB a
//! quarter of the way from the middle to the bottom, −12 dB about 40 % of
//! it, −24 dB about 60 %) and
//! reaches −60 dB, silence ("Off"), at the bottom. The curve meets the
//! straight part at the middle with the same slope, so the line moves
//! evenly through 0 dB.
//!
//! Which tracks show the line is how the timeline is looked at (view state
//! the page keeps), never an edit.

use egui::{Color32, FontId, Pos2, Rect, Stroke, pos2, vec2};

/// The top of the volume range, in decibels.
pub const LOUDEST_DB: f32 = 12.0;
/// The bottom of the volume range, in decibels: silence ("Off").
pub const SILENT_DB: f32 = -60.0;
/// The line can be grabbed this many points above or below it.
pub const GRAB: f32 = 4.0;
/// With Alt held a drag moves the volume this much slower.
pub const FINE: f32 = 0.2;
/// The line's colour: it must stand out from the waveform and the white
/// lines of the fades on every track colour.
const LINE: Color32 = Color32::from_rgb(0xF2, 0xC9, 0x4C);

/// Decibels the curve below the middle falls at its start (per unit of the
/// way down), matching the straight part above.
const SLOPE: f32 = LOUDEST_DB;
/// What the curve adds on top of that, so the bottom is −60 dB.
const BEND: f32 = -SILENT_DB - SLOPE;

/// The volume in decibels at `height` (0 at the bottom of the clip, 1 at
/// its top). The bottom is [`SILENT_DB`].
pub fn db_at(height: f32) -> f32 {
    let height = height.clamp(0.0, 1.0);
    if height >= 0.5 {
        (height - 0.5) * 2.0 * LOUDEST_DB
    } else {
        let down = (0.5 - height) * 2.0;
        -(SLOPE * down + BEND * down * down)
    }
}

/// Where the line for `db` sits: 0 at the bottom of the clip, 1 at its top.
/// The inverse of [`db_at`].
pub fn height_of(db: f32) -> f32 {
    let db = db.clamp(SILENT_DB, LOUDEST_DB);
    if db >= 0.0 {
        0.5 + db / LOUDEST_DB / 2.0
    } else {
        // Solves BEND·d² + SLOPE·d + db = 0 for the way down d.
        let down = (-SLOPE + (SLOPE * SLOPE - 4.0 * BEND * db).sqrt()) / (2.0 * BEND);
        0.5 - down.clamp(0.0, 1.0) / 2.0
    }
}

/// A clip's volume in decibels from its factor (the waveform's scale):
/// [`SILENT_DB`] for silence, within the range otherwise.
pub fn db_of_gain(gain: f32) -> f32 {
    if gain <= 0.0 || !gain.is_finite() {
        SILENT_DB
    } else {
        (20.0 * gain.log10()).clamp(SILENT_DB, LOUDEST_DB)
    }
}

/// The factor for `db`: nothing at all at the bottom of the range.
pub fn gain_of_db(db: f32) -> f32 {
    if db <= SILENT_DB {
        0.0
    } else {
        10f32.powf(db / 20.0)
    }
}

/// The value a drag stops on at `height`: tenths of a decibel, 0 dB
/// within half a decibel (as easy to find as the fader's), and the very
/// bottom silent.
pub fn settled_db(height: f32) -> f32 {
    let db = db_at(height);
    if db <= SILENT_DB + 0.05 {
        SILENT_DB
    } else if db.abs() < 0.5 {
        0.0
    } else {
        (db * 10.0).round() / 10.0
    }
}

/// A volume as the interface writes it: "Off", "0.0 dB", "+3.0 dB", "−6.5 dB".
pub fn words(db: f64) -> String {
    if db <= f64::from(SILENT_DB) {
        "Off".to_owned()
    } else if db.abs() < 0.05 {
        "0.0 dB".to_owned()
    } else {
        let sign = if db < 0.0 { "−" } else { "+" };
        format!("{sign}{:.1} dB", db.abs())
    }
}

/// The part of a clip drawn in `clip` that the line moves in: all of it
/// but a few points at the top and bottom, so both ends stay in reach.
pub fn travel(clip: Rect) -> Rect {
    let inset = (clip.height() * 0.08).clamp(1.0, 4.0);
    Rect::from_min_max(
        pos2(clip.left(), clip.top() + inset),
        pos2(clip.right(), clip.bottom() - inset),
    )
}

/// The line's y for `db` on a clip drawn in `clip`.
pub fn y_of(clip: Rect, db: f32) -> f32 {
    let t = travel(clip);
    t.bottom() - height_of(db) * t.height()
}

/// Whether the pointer at `y` is on the line of a clip drawn in `clip`.
pub fn grabs(clip: Rect, db: f32, y: f32) -> bool {
    clip.height() >= 8.0 && (y - y_of(clip, db)).abs() <= GRAB
}

/// A level line being dragged.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct LevelDrag {
    pub clip: u64,
    /// The volume the clip had when the drag began.
    pub from_db: f32,
    /// Where the line is (0 bottom, 1 top), unrounded, so slow and fine
    /// moves add up.
    pub height: f32,
    /// The pointer's y on the last frame.
    pub last_y: f32,
    /// How far the line moves from bottom to top, in points.
    pub travel: f32,
    /// What letting go now would set.
    pub db: f32,
}

impl LevelDrag {
    /// Grabbed at `y` on a clip drawn in `clip` whose volume is `db`.
    pub fn new(clip_id: u64, clip: Rect, db: f32, y: f32) -> LevelDrag {
        LevelDrag {
            clip: clip_id,
            from_db: db,
            height: height_of(db),
            last_y: y,
            travel: travel(clip).height(),
            db,
        }
    }

    /// The pointer moved to `y`; `fine` (Alt held) moves the line more slowly.
    pub fn follow(&mut self, y: f32, fine: bool) {
        let scale = if fine { FINE } else { 1.0 };
        self.height = (self.height + (self.last_y - y) / self.travel.max(1.0) * scale).clamp(0.0, 1.0);
        self.last_y = y;
        self.db = settled_db(self.height);
    }
}

/// Draws the line for `db` across a clip drawn in `clip`; `lit` while it is
/// pointed at or dragged. `label_x`, while dragging, puts the value beside
/// the pointer.
pub(super) fn draw(p: &egui::Painter, clip: Rect, db: f32, lit: bool, label_x: Option<f32>) {
    let y = y_of(clip, db);
    let width = if lit { 2.5 } else { 2.0 };
    // A dark edge under it keeps it readable over the waveform.
    p.line_segment(
        [pos2(clip.left(), y + 1.0), pos2(clip.right(), y + 1.0)],
        Stroke::new(width, Color32::from_black_alpha(110)),
    );
    p.line_segment(
        [pos2(clip.left(), y), pos2(clip.right(), y)],
        Stroke::new(width, LINE),
    );
    let Some(x) = label_x else { return };
    let text = words(f64::from(db));
    let font = FontId::proportional(11.0);
    let galley = p.layout_no_wrap(text, font, Color32::from_gray(20));
    let size = galley.size() + vec2(10.0, 4.0);
    // Above the line, or under it when it is near the top.
    let top = if y - size.y - 4.0 >= clip.top() {
        y - size.y - 4.0
    } else {
        y + 4.0
    };
    let left = (x + 8.0).min(clip.right() - size.x).max(clip.left());
    let tag = Rect::from_min_size(pos2(left, top), size);
    p.rect_filled(tag, 3.0, LINE);
    p.galley(tag.center() - galley.size() / 2.0, galley, Color32::from_gray(20));
}

/// The level button's picture: a line with a round handle on it.
pub(super) fn draw_icon(p: &egui::Painter, c: Pos2, colour: Color32) {
    p.line_segment(
        [c + vec2(-6.5, 1.5), c + vec2(6.5, 1.5)],
        Stroke::new(1.4, colour),
    );
    p.circle_filled(c + vec2(0.0, 1.5), 2.6, colour);
    // Up and down marks: the line moves that way.
    for side in [-1.0, 1.0] {
        let tip = c + vec2(-4.5, 1.5 + side * 5.0);
        p.line_segment([tip + vec2(-1.8, -side * 1.8), tip], Stroke::new(1.0, colour));
        p.line_segment([tip + vec2(1.8, -side * 1.8), tip], Stroke::new(1.0, colour));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f32, b: f32, within: f32) -> bool {
        (a - b).abs() <= within
    }

    #[test]
    fn the_middle_is_0_db_the_top_12_and_the_bottom_silent() {
        assert_eq!(db_at(0.5), 0.0);
        assert_eq!(db_at(1.0), LOUDEST_DB);
        assert_eq!(db_at(0.0), SILENT_DB);
        assert_eq!(db_at(0.75), 6.0, "straight decibels above the middle");
        // Past the ends is the ends.
        assert_eq!(db_at(1.7), LOUDEST_DB);
        assert_eq!(db_at(-0.2), SILENT_DB);
    }

    #[test]
    fn the_usual_cuts_have_room_below_the_middle() {
        // -6 dB a quarter of the way down, -12 and -24 dB further on.
        assert!(close(height_of(-6.0), 0.375, 1e-4), "{}", height_of(-6.0));
        assert!(close(height_of(-12.0), 0.3048, 1e-3), "{}", height_of(-12.0));
        assert!(close(height_of(-24.0), 0.2034, 1e-3), "{}", height_of(-24.0));
        // Each is well apart from the next: easy to hit with a mouse.
        let gaps = [0.0, -6.0, -12.0, -24.0, -60.0].map(height_of);
        for pair in gaps.windows(2) {
            assert!(pair[0] - pair[1] > 0.06, "{gaps:?}");
        }
    }

    #[test]
    fn height_and_volume_turn_into_each_other() {
        for db in [-60.0, -40.0, -24.0, -12.0, -6.0, -3.0, -0.5, 0.0, 0.5, 3.0, 12.0] {
            assert!(close(db_at(height_of(db)), db, 1e-3), "{db}");
        }
        for i in 0..=100 {
            let h = i as f32 / 100.0;
            assert!(close(height_of(db_at(h)), h, 1e-4), "{h}");
        }
        // Always higher for louder, and no jump at the middle.
        let mut last = SILENT_DB - 1.0;
        for i in 0..=1000 {
            let db = db_at(i as f32 / 1000.0);
            assert!(db > last, "rises everywhere");
            assert!(db - last < 0.5 || i == 0, "no jump at {i}");
            last = db;
        }
    }

    #[test]
    fn a_drag_stops_on_tenths_finds_0_db_and_reaches_off() {
        assert_eq!(settled_db(0.5 + 0.01), 0.0, "0 dB is easy to find");
        assert_eq!(settled_db(height_of(-3.04)), -3.0);
        assert_eq!(settled_db(0.0), SILENT_DB);
        assert_eq!(settled_db(1.0), LOUDEST_DB);
    }

    #[test]
    fn volumes_read_as_the_inspector_writes_them() {
        assert_eq!(words(-60.0), "Off");
        assert_eq!(words(0.02), "0.0 dB");
        assert_eq!(words(-3.0), "−3.0 dB");
        assert_eq!(words(2.5), "+2.5 dB");
    }

    #[test]
    fn factors_and_decibels() {
        assert_eq!(db_of_gain(0.0), SILENT_DB);
        assert_eq!(db_of_gain(1.0), 0.0);
        assert!(close(db_of_gain(0.5), -6.02, 0.01));
        assert_eq!(gain_of_db(SILENT_DB), 0.0);
        assert!(close(gain_of_db(-6.0206), 0.5, 1e-4));
    }

    #[test]
    fn the_line_sits_in_the_middle_at_0_db_and_is_grabbed_near_it() {
        let clip = Rect::from_min_max(pos2(0.0, 100.0), pos2(200.0, 150.0));
        assert_eq!(y_of(clip, 0.0), 125.0);
        assert!(y_of(clip, LOUDEST_DB) > clip.top() && y_of(clip, SILENT_DB) < clip.bottom());
        assert!(grabs(clip, 0.0, 128.0) && !grabs(clip, 0.0, 131.0));
    }

    #[test]
    fn a_drag_follows_the_pointer_and_alt_makes_it_fine() {
        // A clip 108 points tall: the line travels 100 of them.
        let clip = Rect::from_min_max(pos2(0.0, 246.0), pos2(200.0, 354.0));
        let mut drag = LevelDrag::new(7, clip, 0.0, 300.0);
        assert_eq!(drag.travel, 100.0);
        drag.follow(275.0, false);
        assert_eq!(drag.db, 6.0, "a quarter of the way up");
        drag.follow(300.0, false);
        assert_eq!(drag.db, 0.0);
        // Fine: the same move goes a fifth as far.
        drag.follow(275.0, true);
        assert!(close(drag.db, 1.2, 0.05), "{}", drag.db);
        // Far past the bottom is silence.
        drag.follow(2000.0, false);
        assert_eq!(drag.db, SILENT_DB);
        assert_eq!(drag.from_db, 0.0, "what it was is kept");
    }
}
