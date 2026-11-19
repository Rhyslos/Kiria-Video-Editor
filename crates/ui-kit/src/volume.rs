//! One volume scale for every fader and volume text, so the same volume
//! sits at the same place and reads the same everywhere (working rule 1):
//! the track header's fader on the timeline, the mixer's strips, the
//! master column and the clip's level line.
//!
//! - The range: [`BOTTOM_DB`] to [`TOP_DB`].
//! - The fader law ([`share_of`], [`db_at_share`]): straight decibels from
//!   the bottom to the top, as the meters beside the faders read. (The
//!   level line on a clip has a law of its own, `timeline::level`, that
//!   gives the useful cuts more of the clip's height; its ends and its
//!   text are these.)
//! - Where a drag lands ([`settled`], [`settled_fine`]): 0 dB is easy to
//!   find, the bottom stays the bottom.
//! - What the bottom is ([`Bottom`]) and the text ([`words`], [`number`],
//!   [`read`]): the text says what is heard. A clip's volume at the bottom
//!   of the range is silence (`engine::mix::volume`, `project::Sound::SILENT`)
//!   and reads "Off"; a track's or the whole mix's volume is heard at
//!   −60 dB there (the audio crate's `volume_gain` turns every decibel into
//!   a factor) and reads "−60.0 dB". The volume lines follow their owner:
//!   a clip's line at the bottom is silence, a track's is heard.

/// The top of the range, in decibels. The same as the top of
/// `project::Track::GAINS_DB`; a test in the app crate
/// (`the_volume_range_is_the_same_in_the_project_and_on_screen`) keeps them so.
pub const TOP_DB: f32 = 12.0;
/// The bottom of the range, in decibels. The same as the bottom of
/// `project::Track::GAINS_DB` (see [`TOP_DB`]).
pub const BOTTOM_DB: f32 = -60.0;

/// What the bottom of a volume's range is heard as.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Bottom {
    /// Nothing at all: a clip's volume. Reads "Off".
    Silent,
    /// [`BOTTOM_DB`] like any other volume: a track's volume and the
    /// whole mix's. Reads "−60.0 dB".
    Heard,
}

/// How far up the range `db` is: 0 at the bottom, 1 at the top, straight
/// decibels in between. Past the ends is the ends.
pub fn share_of(db: f32) -> f32 {
    ((db - BOTTOM_DB) / (TOP_DB - BOTTOM_DB)).clamp(0.0, 1.0)
}

/// The volume `share` of the way up the range: the inverse of [`share_of`].
pub fn db_at_share(share: f32) -> f32 {
    BOTTOM_DB + share.clamp(0.0, 1.0) * (TOP_DB - BOTTOM_DB)
}

/// Where a fader dragged to `db` lands: half decibels, 0 dB within one,
/// inside the range.
pub fn settled(db: f32) -> f32 {
    let db = db.clamp(BOTTOM_DB, TOP_DB);
    if db.abs() < 1.0 {
        0.0
    } else {
        (db * 2.0).round() / 2.0
    }
}

/// Where a line dragged to `db` lands (the level line and the volume
/// line, which set finer values than a fader): tenths of a decibel, 0 dB
/// within half of one, the very bottom the bottom.
pub fn settled_fine(db: f32) -> f32 {
    let db = db.clamp(BOTTOM_DB, TOP_DB);
    if db <= BOTTOM_DB + 0.05 {
        BOTTOM_DB
    } else if db.abs() < 0.5 {
        0.0
    } else {
        (db * 10.0).round() / 10.0
    }
}

/// A volume as the interface writes it: "−6.0 dB", "0.0 dB", "+3.0 dB",
/// and at the bottom "Off" where that is silence, "−60.0 dB" where it is
/// heard.
pub fn words(db: f32, bottom: Bottom) -> String {
    let text = number(db, bottom);
    if text == OFF { text } else { format!("{text} dB") }
}

/// [`words`] without the unit, for a column too narrow for it: "−6.0",
/// "0.0", "+3.0", "Off".
pub fn number(db: f32, bottom: Bottom) -> String {
    if bottom == Bottom::Silent && db <= BOTTOM_DB {
        return OFF.to_owned();
    }
    let db = db.max(BOTTOM_DB);
    if db.abs() < 0.05 {
        "0.0".to_owned()
    } else {
        let sign = if db < 0.0 { "−" } else { "+" };
        format!("{sign}{:.1}", db.abs())
    }
}

/// What the bottom of a silent range is called.
const OFF: &str = "Off";

/// A typed volume: "-6", "−6.5 dB", "+3", "0", brought inside the range;
/// "off" (the bottom) only where the bottom is silence.
pub fn read(text: &str, bottom: Bottom) -> Option<f32> {
    let text = text.trim().to_lowercase().replace('−', "-");
    if text == "off" {
        return (bottom == Bottom::Silent).then_some(BOTTOM_DB);
    }
    let number = text.trim_end_matches("db").trim();
    let db: f32 = number.parse().ok()?;
    db.is_finite().then(|| db.clamp(BOTTOM_DB, TOP_DB))
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{Rect, pos2};

    #[test]
    fn the_scale_runs_straight_from_the_bottom_to_the_top() {
        assert_eq!(share_of(BOTTOM_DB), 0.0);
        assert_eq!(share_of(TOP_DB), 1.0);
        assert_eq!(share_of(0.0), 60.0 / 72.0);
        assert_eq!(share_of(-90.0), 0.0);
        assert_eq!(share_of(40.0), 1.0);
        for db in [-60.0, -24.0, -6.0, 0.0, 3.5, 12.0] {
            assert!((db_at_share(share_of(db)) - db).abs() < 1e-4, "{db}");
        }
    }

    #[test]
    fn drags_find_0_db_and_keep_the_ends() {
        assert_eq!(settled(0.7), 0.0);
        assert_eq!(settled(-6.3), -6.5);
        assert_eq!(settled(BOTTOM_DB), BOTTOM_DB);
        assert_eq!(settled(-80.0), BOTTOM_DB);
        assert_eq!(settled(TOP_DB), TOP_DB);
        assert_eq!(settled_fine(0.4), 0.0);
        assert_eq!(settled_fine(-3.04), -3.0);
        assert_eq!(settled_fine(BOTTOM_DB + 0.04), BOTTOM_DB);
        assert_eq!(settled_fine(20.0), TOP_DB);
    }

    #[test]
    fn the_text_says_what_is_heard() {
        assert_eq!(words(BOTTOM_DB, Bottom::Silent), "Off");
        assert_eq!(words(BOTTOM_DB, Bottom::Heard), "−60.0 dB");
        for bottom in [Bottom::Silent, Bottom::Heard] {
            assert_eq!(words(0.02, bottom), "0.0 dB");
            assert_eq!(words(-0.0, bottom), "0.0 dB");
            assert_eq!(words(-8.0, bottom), "−8.0 dB");
            assert_eq!(words(TOP_DB, bottom), "+12.0 dB");
            assert_eq!(number(-8.0, bottom), "−8.0");
        }
        assert_eq!(number(BOTTOM_DB, Bottom::Silent), "Off");
    }

    #[test]
    fn typed_volumes() {
        for bottom in [Bottom::Silent, Bottom::Heard] {
            assert_eq!(read("-6", bottom), Some(-6.0));
            assert_eq!(read("−6.5 dB", bottom), Some(-6.5));
            assert_eq!(read("+3", bottom), Some(3.0));
            assert_eq!(read("40", bottom), Some(TOP_DB));
            assert_eq!(read("loud", bottom), None);
        }
        assert_eq!(read("Off", Bottom::Silent), Some(BOTTOM_DB));
        assert_eq!(
            read("Off", Bottom::Heard),
            None,
            "a track at the bottom is still heard"
        );
    }

    /// Every place that shows a volume puts it at the same share of its
    /// fader and writes it the same way, the bottom, 0 dB and the top too.
    #[test]
    fn every_fader_agrees() {
        let column = Rect::from_min_max(pos2(0.0, 100.0), pos2(20.0, 820.0));
        let row = Rect::from_min_max(pos2(30.0, 0.0), pos2(210.0, 5.0));
        let up = |y: f32| (column.bottom() - y) / column.height();
        let along = |x: f32| (x - row.left()) / row.width();
        for db in [BOTTOM_DB, -48.0, -6.0, -0.5, 0.0, 0.5, 6.0, TOP_DB] {
            let share = share_of(db);
            // Master column and mixer strips: the same y.
            assert!((up(crate::master::y_of(column, db)) - share).abs() < 1e-5, "{db}");
            // The track header's fader on the timeline.
            assert!(
                (along(crate::timeline::fader_x(row, db)) - share).abs() < 1e-5,
                "{db}"
            );
            // A drag from that place lands at the same volume.
            let master_drag = settled(crate::master::db_at(column, crate::master::y_of(column, db)));
            let header_drag = crate::timeline::fader_db_at(row, crate::timeline::fader_x(row, db));
            assert_eq!(master_drag, header_drag, "{db}");
            // The track's text: the mixer's box, the header's tip and the
            // master column's row.
            let track = words(db, Bottom::Heard);
            assert_eq!(crate::mixer::db_words(db), track);
            assert!(crate::timeline::fader_tip(db).starts_with(&format!("Volume {track} ")));
            assert_eq!(crate::master::volume_text(db), number(db, Bottom::Heard));
            // A clip's text: the level line's.
            assert_eq!(
                crate::timeline::level::words(f64::from(db)),
                words(db, Bottom::Silent)
            );
            // The volume lines: a track's as the track, a clip's as the clip.
            let volume_line = crate::timeline::LineKind::Volume;
            assert_eq!(volume_line.words(db, crate::timeline::LineOwner::Track(1)), track);
            assert_eq!(
                volume_line.words(db, crate::timeline::LineOwner::Clip(1)),
                words(db, Bottom::Silent)
            );
        }
        assert_eq!(crate::mixer::db_words(BOTTOM_DB), "−60.0 dB");
        assert_eq!(crate::timeline::level::words(f64::from(BOTTOM_DB)), "Off");
        assert_eq!(
            crate::timeline::LineKind::Volume.words(BOTTOM_DB, crate::timeline::LineOwner::Track(1)),
            "−60.0 dB",
            "a track's line at the bottom is still heard"
        );
        assert_eq!(
            crate::timeline::LineKind::Volume.words(BOTTOM_DB, crate::timeline::LineOwner::Clip(1)),
            "Off"
        );
        // The level line's own law shares the ends: the top at the top,
        // the bottom (silence for a clip) in its bottom few hundredths.
        assert_eq!(crate::timeline::level::db_at(0.0), BOTTOM_DB);
        assert!(crate::timeline::level::height_of(BOTTOM_DB) < 0.025);
        assert_eq!(crate::timeline::level::height_of(TOP_DB), 1.0);
    }
}
