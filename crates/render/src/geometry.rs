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

/// A whole picture's place in the frame: where its centre is, how large it
/// is drawn and how far it is turned.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Placement {
    pub centre: Point,
    /// Width and height in frame pixels, before turning.
    pub size: (f32, f32),
    /// Clockwise turn around the centre.
    pub degrees: f32,
}

/// Quick colour, as the numbers the shader works with (in linear light).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Colour {
    /// What red, green and blue are multiplied by: exposure, temperature and tint.
    pub gains: [f32; 3],
    /// The power values are raised to around middle grey: over 1 is harder, under 1 flatter.
    pub contrast: f32,
    /// 0 is no colour, 1 leaves it, 2 is twice the colour.
    pub saturation: f32,
}

impl Colour {
    /// Leaves the picture as it is.
    pub const NEUTRAL: Colour = Colour {
        gains: [1.0; 3],
        contrast: 1.0,
        saturation: 1.0,
    };

    /// From what the sliders say: `exposure` in stops, the others from -1
    /// to 1 with 0 leaving the picture alone. Warmer is more red and less
    /// blue, a higher tint is more magenta (less green); neither changes
    /// how bright a grey is.
    pub fn from_sliders(
        exposure: f32,
        contrast: f32,
        saturation: f32,
        temperature: f32,
        tint: f32,
    ) -> Colour {
        let (red, green, blue) = (
            2f32.powf(0.5 * temperature),
            2f32.powf(-0.5 * tint),
            2f32.powf(-0.5 * temperature),
        );
        let brightness = WEIGHTS[0] * red + WEIGHTS[1] * green + WEIGHTS[2] * blue;
        let gain = 2f32.powf(exposure) / brightness;
        Colour {
            gains: [red * gain, green * gain, blue * gain],
            contrast: 2f32.powf(contrast),
            saturation: (1.0 + saturation).max(0.0),
        }
    }

    pub fn is_neutral(&self) -> bool {
        *self == Colour::NEUTRAL
    }
}

/// How much red, green and blue count towards brightness (BT.709).
pub const WEIGHTS: [f32; 3] = [0.2126, 0.7152, 0.0722];

impl Area {
    /// Where `part` of a picture placed at `placement` lands: the part keeps
    /// its place within the whole picture, so cutting a side off moves
    /// nothing else.
    pub fn of_part(placement: Placement, part: Part) -> Area {
        let (w, h) = placement.size;
        let (sin, cos) = placement.degrees.to_radians().sin_cos();
        // Clockwise on a screen, where y runs down.
        let turn = |(x, y): Point| (x * cos - y * sin, x * sin + y * cos);
        let corner = turn((w * (part.left - 0.5), h * (part.top - 0.5)));
        Area {
            origin: (placement.centre.0 + corner.0, placement.centre.1 + corner.1),
            across: turn((w * part.width, 0.0)),
            down: turn((0.0, h * part.height)),
        }
    }

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
    fn a_placed_picture_is_centred_where_it_is_put_and_turns_clockwise() {
        let placement = Placement {
            centre: (100.0, 50.0),
            size: (40.0, 20.0),
            degrees: 0.0,
        };
        let area = Area::of_part(placement, Part::WHOLE);
        assert_eq!(
            (area.origin, area.across, area.down),
            ((80.0, 40.0), (40.0, 0.0), (0.0, 20.0))
        );
        // A quarter turn clockwise: the top edge now runs down the right side.
        let turned = Area::of_part(
            Placement {
                degrees: 90.0,
                ..placement
            },
            Part::WHOLE,
        );
        let near = |a: Point, b: Point| (a.0 - b.0).abs() < 1e-3 && (a.1 - b.1).abs() < 1e-3;
        assert!(near(turned.origin, (110.0, 30.0)), "{turned:?}");
        assert!(
            near(turned.across, (0.0, 40.0)) && near(turned.down, (-20.0, 0.0)),
            "{turned:?}"
        );
    }

    #[test]
    fn a_part_keeps_its_place_within_the_whole_picture() {
        let placement = Placement {
            centre: (100.0, 50.0),
            size: (40.0, 20.0),
            degrees: 0.0,
        };
        // A quarter cut off the left: the right edge stays where it was.
        let part = Part {
            left: 0.25,
            top: 0.0,
            width: 0.75,
            height: 1.0,
        };
        let area = Area::of_part(placement, part);
        assert_eq!(
            (area.origin, area.across, area.down),
            ((90.0, 40.0), (30.0, 0.0), (0.0, 20.0))
        );
    }

    #[test]
    fn quick_colour_at_rest_is_neutral_and_warmth_keeps_greys_as_bright() {
        assert!(Colour::from_sliders(0.0, 0.0, 0.0, 0.0, 0.0).is_neutral());
        let warm = Colour::from_sliders(0.0, 0.0, 0.0, 1.0, 0.0);
        assert!(warm.gains[0] > 1.0 && warm.gains[2] < 1.0);
        let brightness: f32 = warm
            .gains
            .iter()
            .zip(WEIGHTS)
            .map(|(gain, weight)| gain * weight)
            .sum();
        assert!((brightness - 1.0).abs() < 1e-5);
        let brighter = Colour::from_sliders(1.0, 0.0, 0.0, 0.0, 0.0);
        assert!(
            (brighter.gains[1] - 2.0).abs() < 1e-5,
            "a stop is twice the light"
        );
        assert_eq!(Colour::from_sliders(0.0, 0.0, -1.0, 0.0, 0.0).saturation, 0.0);
    }

    #[test]
    fn an_area_scaled_with_its_frame_keeps_its_place_in_it() {
        let area = Area::fitted((3440, 1440), (1920, 1080)).scaled(0.5, 0.5);
        assert_eq!(area.across, (960.0, 0.0));
        assert!((area.origin.1 - (540.0 - 960.0 * 1440.0 / 3440.0) / 2.0).abs() < 0.01);
    }
}
