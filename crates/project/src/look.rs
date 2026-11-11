//! How a clip's picture is placed, cut and coloured.
//!
//! Every value is a whole number of thousandths (of the frame, of the
//! picture, of a degree, of a stop), so two clips compare exactly and a file
//! saved and opened again says the same. Convert to floats only to draw.

use diag::AppError;
use serde::{Deserialize, Serialize};

/// One whole, in the thousandths every value here is counted in.
pub const ONE: i32 = 1000;

/// A clip's picture in the frame: where, how large, which part, how strong,
/// and its quick colour. The default leaves the picture as it is: fitted in
/// the frame, centred, whole and opaque.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(default)]
pub struct Look {
    /// The picture's centre, from the frame's centre: thousandths of the
    /// frame's width to the right and of its height down.
    pub x: i32,
    pub y: i32,
    /// Size, in thousandths of the size that just fits the frame.
    pub scale: i32,
    /// Turn around the picture's centre, in thousandths of a degree clockwise.
    pub turn: i32,
    /// Cut off each side, in thousandths of the picture's width or height.
    pub crop_left: i32,
    pub crop_top: i32,
    pub crop_right: i32,
    pub crop_bottom: i32,
    /// 1000 hides what is below, 0 can't be seen.
    pub opacity: i32,
    /// Brighter or darker, in thousandths of a stop.
    pub exposure: i32,
    /// -1000 (flat) to 1000 (hard); 0 leaves it.
    pub contrast: i32,
    /// -1000 (no colour) to 1000 (twice the colour); 0 leaves it.
    pub saturation: i32,
    /// -1000 (cooler, bluer) to 1000 (warmer); 0 leaves it.
    pub temperature: i32,
    /// -1000 (greener) to 1000 (more magenta); 0 leaves it.
    pub tint: i32,
}

impl Default for Look {
    fn default() -> Look {
        Look::PLAIN
    }
}

impl Look {
    /// The picture as it is.
    pub const PLAIN: Look = Look {
        x: 0,
        y: 0,
        scale: ONE,
        turn: 0,
        crop_left: 0,
        crop_top: 0,
        crop_right: 0,
        crop_bottom: 0,
        opacity: ONE,
        exposure: 0,
        contrast: 0,
        saturation: 0,
        temperature: 0,
        tint: 0,
    };

    /// The smallest and largest size: 1% to 100 times the fitted size.
    pub const SCALE: (i32, i32) = (10, 100 * ONE);
    /// How far the centre may be from the frame's centre: ten frames.
    pub const AWAY: i32 = 10 * ONE;
    /// The most that can be cut off the two opposite sides together: a
    /// hundredth of the picture always stays.
    pub const MOST_CROP: i32 = ONE - 10;
    /// Five stops either way.
    pub const EXPOSURE: i32 = 5 * ONE;

    /// Whether the picture is moved, resized, turned or cut.
    pub fn is_placed(&self) -> bool {
        (self.x, self.y, self.scale, self.turn) != (0, 0, ONE, 0) || self.is_cropped()
    }

    pub fn is_cropped(&self) -> bool {
        (self.crop_left, self.crop_top, self.crop_right, self.crop_bottom) != (0, 0, 0, 0)
    }

    /// Whether the quick colour changes anything.
    pub fn is_coloured(&self) -> bool {
        (
            self.exposure,
            self.contrast,
            self.saturation,
            self.temperature,
            self.tint,
        ) != (0, 0, 0, 0, 0)
    }

    /// The same look with every value brought inside its range: what a
    /// slider dragged too far means.
    pub fn within_limits(self) -> Look {
        let crop = |near: i32, far: i32| {
            let near = near.clamp(0, Look::MOST_CROP);
            (near, far.clamp(0, Look::MOST_CROP - near))
        };
        let (crop_left, crop_right) = crop(self.crop_left, self.crop_right);
        let (crop_top, crop_bottom) = crop(self.crop_top, self.crop_bottom);
        Look {
            x: self.x.clamp(-Look::AWAY, Look::AWAY),
            y: self.y.clamp(-Look::AWAY, Look::AWAY),
            scale: self.scale.clamp(Look::SCALE.0, Look::SCALE.1),
            // A full turn is no turn.
            turn: self.turn % (360 * ONE),
            crop_left,
            crop_top,
            crop_right,
            crop_bottom,
            opacity: self.opacity.clamp(0, ONE),
            exposure: self.exposure.clamp(-Look::EXPOSURE, Look::EXPOSURE),
            contrast: self.contrast.clamp(-ONE, ONE),
            saturation: self.saturation.clamp(-ONE, ONE),
            temperature: self.temperature.clamp(-ONE, ONE),
            tint: self.tint.clamp(-ONE, ONE),
        }
    }

    /// Refuses, explained, a look with a value outside its range.
    pub fn check(&self) -> diag::Result<()> {
        if *self == self.within_limits() {
            return Ok(());
        }
        Err(AppError::new("That picture setting can't be used")
            .why("One of its values is outside its range (for example a size under 1% or more cut off than the picture has).")
            .fix("Reset the clip's Transform, Crop or Quick color in the inspector and set it again."))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_changes_nothing() {
        let look = Look::default();
        assert_eq!(look, Look::PLAIN);
        assert!(!look.is_placed() && !look.is_cropped() && !look.is_coloured());
        assert!(look.check().is_ok());
    }

    #[test]
    fn values_out_of_range_are_brought_back_and_refused_as_they_are() {
        let wild = Look {
            scale: 0,
            opacity: 1500,
            crop_left: 700,
            crop_right: 700,
            turn: 370 * ONE,
            exposure: -9000,
            ..Look::PLAIN
        };
        assert!(wild.check().is_err());
        let tamed = wild.within_limits();
        assert_eq!(tamed.scale, Look::SCALE.0);
        assert_eq!(tamed.opacity, ONE);
        assert_eq!(
            (tamed.crop_left, tamed.crop_right),
            (700, 290),
            "a hundredth stays"
        );
        assert_eq!(tamed.turn, 10 * ONE);
        assert_eq!(tamed.exposure, -Look::EXPOSURE);
        assert!(tamed.check().is_ok());
    }

    #[test]
    fn what_changed_is_told_apart() {
        let moved = Look {
            x: 100,
            ..Look::PLAIN
        };
        assert!(moved.is_placed() && !moved.is_coloured());
        let cut = Look {
            crop_top: 100,
            ..Look::PLAIN
        };
        assert!(cut.is_placed() && cut.is_cropped());
        let warm = Look {
            temperature: 200,
            ..Look::PLAIN
        };
        assert!(warm.is_coloured() && !warm.is_placed());
    }

    #[test]
    fn a_file_without_some_values_reads_them_as_plain() {
        // What a later format may leave out, or an earlier one never had.
        let look: Look = ron::from_str("(x: 250, opacity: 500)").unwrap();
        assert_eq!(
            look,
            Look {
                x: 250,
                opacity: 500,
                ..Look::PLAIN
            }
        );
    }
}
