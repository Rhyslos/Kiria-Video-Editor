//! Where things are in a frame: plain numbers in frame pixels, x to the
//! right and y down, worked out without a graphics card.

/// A point or a step in the frame, in frame pixels: x to the right, y down.
pub type Point = (f32, f32);

/// A rectangle in the frame that may be turned: its top left corner and its
/// two edges from there.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Area {
    pub origin: Point,
    /// From the top left corner to the top right.
    pub across: Point,
    /// From the top left corner to the bottom left.
    pub down: Point,
}

/// A part of a picture: its left and top edge and its size, each in 0-1.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Part {
    pub left: f32,
    pub top: f32,
    pub width: f32,
    pub height: f32,
}

impl Part {
    pub const WHOLE: Part = Part {
        left: 0.0,
        top: 0.0,
        width: 1.0,
        height: 1.0,
    };
}

impl Area {
    /// The largest upright rectangle of a picture's shape that fits in the
    /// frame, centred: bars at the sides or above and below when the shapes
    /// differ, never a cut-off or stretched picture.
    pub fn fitted(picture: (u32, u32), frame: (u32, u32)) -> Area {
        let (pw, ph) = (picture.0.max(1) as f32, picture.1.max(1) as f32);
        let (fw, fh) = (frame.0 as f32, frame.1 as f32);
        // The side that limits the size is the frame's own, exactly.
        let (w, h) = if fw * ph <= fh * pw {
            (fw, ph * fw / pw)
        } else {
            (pw * fh / ph, fh)
        };
        Area {
            origin: ((fw - w) / 2.0, (fh - h) / 2.0),
            across: (w, 0.0),
            down: (0.0, h),
        }
    }

    /// The same area in a frame `x` times as wide and `y` times as high.
    pub fn scaled(self, x: f32, y: f32) -> Area {
        let scale = |(px, py): Point| (px * x, py * y);
        Area {
            origin: scale(self.origin),
            across: scale(self.across),
            down: scale(self.down),
        }
    }

    /// The lengths of its two edges, in frame pixels.
    pub fn size(&self) -> (f32, f32) {
        (self.across.0.hypot(self.across.1), self.down.0.hypot(self.down.1))
    }

    /// Whether nothing of a frame of `frame` pixels shows past this area.
    /// Only said of upright areas: a turned one is taken not to.
    pub fn covers(&self, frame: (u32, u32)) -> bool {
        // Half a pixel of slack: a fitted picture may miss the edge by rounding.
        const SLACK: f32 = 0.5;
        let upright = self.across.1 == 0.0 && self.down.0 == 0.0;
        let (left, right) = ends(self.origin.0, self.across.0);
        let (top, bottom) = ends(self.origin.1, self.down.1);
        upright
            && left <= SLACK
            && top <= SLACK
            && right >= frame.0 as f32 - SLACK
            && bottom >= frame.1 as f32 - SLACK
    }
}

/// The lower and higher end of a side that starts at `from` and goes `by`.
fn ends(from: f32, by: f32) -> (f32, f32) {
    (from.min(from + by), from.max(from + by))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_picture_of_the_frames_shape_fills_it() {
        let area = Area::fitted((2560, 1440), (1920, 1080));
        assert_eq!(area.origin, (0.0, 0.0));
        assert_eq!(area.across, (1920.0, 0.0));
        assert_eq!(area.down, (0.0, 1080.0));
        assert!(area.covers((1920, 1080)));
    }

    #[test]
    fn a_wider_picture_gets_bars_above_and_below() {
        // 21:9 into 16:9.
        let area = Area::fitted((3440, 1440), (1920, 1080));
        assert_eq!(area.across, (1920.0, 0.0));
        let height = 1920.0 * 1440.0 / 3440.0;
        assert!((area.down.1 - height).abs() < 0.01);
        assert!((area.origin.1 - (1080.0 - height) / 2.0).abs() < 0.01);
        assert!(!area.covers((1920, 1080)));
    }

    #[test]
    fn a_wide_picture_in_a_tall_frame_sits_in_the_middle() {
        let area = Area::fitted((1920, 1080), (1080, 1920));
        assert_eq!(area.across, (1080.0, 0.0));
        assert!((area.down.1 - 607.5).abs() < 0.01);
        assert!((area.origin.1 - 656.25).abs() < 0.01);
    }

    #[test]
    fn a_turned_area_never_counts_as_covering() {
        let mut area = Area::fitted((1920, 1080), (1920, 1080));
        area.across.1 = 1.0;
        assert!(!area.covers((1920, 1080)));
    }

    #[test]
    fn an_area_scaled_with_its_frame_keeps_its_place_in_it() {
        let area = Area::fitted((3440, 1440), (1920, 1080)).scaled(0.5, 0.5);
        assert_eq!(area.across, (960.0, 0.0));
        assert!((area.origin.1 - (540.0 - 960.0 * 1440.0 / 3440.0) / 2.0).abs() < 0.01);
    }
}
