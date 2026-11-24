//! Finding black bars: the stretches along a recording's edges that are
//! black in every frame looked at, as when a game drawn at 2560×1440 is
//! recorded on a 3440×1440 screen, or a 4:3 picture sits in a 16:9 one.
//! What is found becomes the clip's ordinary crop (the inspector's "Black
//! bars"); nothing about the finding is kept.

use media::{Decoding, VideoDecoder};

/// How many frames are looked at, spread over the stretch.
const FRAMES: usize = 9;

/// Brightest a pixel of a bar may be (0-255 per channel): black as a
/// recorder stores it is 16 or less, and a little noise is allowed for.
const DARK: u8 = 28;

/// A bar thinner than this share of the side is not taken as one (a dark
/// line at the edge of the picture itself).
const THINNEST: f64 = 0.01;

/// What was found, in pixels of the upright picture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Bars {
    /// The upright picture's size.
    pub picture: (u32, u32),
    /// Black along the left, top, right and bottom edges.
    pub left: u32,
    pub top: u32,
    pub right: u32,
    pub bottom: u32,
}

impl Bars {
    /// Whether any bar was found.
    pub fn any(&self) -> bool {
        self.left + self.top + self.right + self.bottom > 0
    }

    /// The size of what is left between the bars.
    pub fn inside(&self) -> (u32, u32) {
        (
            self.picture.0.saturating_sub(self.left + self.right),
            self.picture.1.saturating_sub(self.top + self.bottom),
        )
    }

    /// The bars as shares of the picture's width and height: left, top,
    /// right, bottom.
    pub fn shares(&self) -> [f64; 4] {
        let (w, h) = (f64::from(self.picture.0.max(1)), f64::from(self.picture.1.max(1)));
        [
            f64::from(self.left) / w,
            f64::from(self.top) / h,
            f64::from(self.right) / w,
            f64::from(self.bottom) / h,
        ]
    }
}

/// Looks at frames of stream `stream` of `path` between `from` and `to`
/// (seconds of the file) for black bars. Takes a moment: run it on a worker.
pub fn find(path: &std::path::Path, stream: u32, rotation: u16, from: f64, to: f64) -> diag::Result<Bars> {
    let mut decoder = VideoDecoder::open(path, stream, rotation, Decoding::Best)?;
    let span = (to - from).max(0.0);
    let mut brightest: Option<Brightest> = None;
    for i in 0..FRAMES {
        let at = from + span * (i as f64 + 0.5) / FRAMES as f64;
        let frame = decoder.frame_at(at, u32::MAX, u32::MAX)?;
        let seen = brightest.get_or_insert_with(|| Brightest::new(frame.width, frame.height));
        if (seen.width, seen.height) == (frame.width, frame.height) {
            seen.add(&frame.rgba);
        }
    }
    Ok(brightest.map_or(
        Bars {
            picture: (0, 0),
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        },
        |b| b.bars(),
    ))
}

/// The brightest each column and row has been, over the frames looked at.
struct Brightest {
    width: u32,
    height: u32,
    columns: Vec<u8>,
    rows: Vec<u8>,
}

impl Brightest {
    fn new(width: u32, height: u32) -> Brightest {
        Brightest {
            width,
            height,
            columns: vec![0; width as usize],
            rows: vec![0; height as usize],
        }
    }

    fn add(&mut self, rgba: &[u8]) {
        let width = self.width as usize;
        for (y, row) in rgba.chunks_exact(width * 4).enumerate() {
            for (x, pixel) in row.chunks_exact(4).enumerate() {
                let bright = pixel[0].max(pixel[1]).max(pixel[2]);
                self.columns[x] = self.columns[x].max(bright);
                self.rows[y] = self.rows[y].max(bright);
            }
        }
    }

    fn bars(&self) -> Bars {
        let (left, right) = edges(&self.columns);
        let (top, bottom) = edges(&self.rows);
        snapped(Bars {
            picture: (self.width, self.height),
            left,
            top,
            right,
            bottom,
        })
    }
}

/// The usual shapes a picture inside bars has (wide to high).
const SHAPES: [(u32, u32); 6] = [(16, 9), (64, 27), (4, 3), (16, 10), (1, 1), (9, 16)];

/// How near (a share of the side) the picture between the bars must be to
/// a usual shape to be taken as it: a compressed recording smears the edge
/// of the picture into the bars by a few pixels.
const NEAR_SHAPE: f64 = 0.015;

/// Bars on the two sides (or top and bottom) that leave nearly a usual
/// shape are made to leave it exactly, the same on both sides: 1288×720
/// found in a 1720×720 recording is the 1280×720 picture it holds.
fn snapped(bars: Bars) -> Bars {
    let (w, h) = bars.picture;
    let (inside_w, inside_h) = bars.inside();
    let near = |found: u32, exact: u32| f64::from(found.abs_diff(exact)) <= f64::from(exact) * NEAR_SHAPE;
    for (wide, high) in SHAPES {
        if bars.top + bars.bottom == 0 && bars.left + bars.right > 0 {
            let exact = ((u64::from(h) * u64::from(wide) / u64::from(high)) as u32) & !1;
            if exact < w && near(inside_w, exact) && bars.left.abs_diff(bars.right) <= w / 50 {
                let side = (w - exact) / 2;
                return Bars {
                    left: side,
                    right: w - exact - side,
                    ..bars
                };
            }
        }
        if bars.left + bars.right == 0 && bars.top + bars.bottom > 0 {
            let exact = ((u64::from(w) * u64::from(high) / u64::from(wide)) as u32) & !1;
            if exact < h && near(inside_h, exact) && bars.top.abs_diff(bars.bottom) <= h / 50 {
                let side = (h - exact) / 2;
                return Bars {
                    top: side,
                    bottom: h - exact - side,
                    ..bars
                };
            }
        }
    }
    bars
}

/// How many lines along each end of `brightest` stayed dark: each made
/// even (so the picture left is too) and dropped when too thin to be a bar.
/// A picture dark all through has no bars.
fn edges(brightest: &[u8]) -> (u32, u32) {
    let len = brightest.len();
    let Some(first) = brightest.iter().position(|b| *b > DARK) else {
        return (0, 0);
    };
    let last = brightest.iter().rposition(|b| *b > DARK).unwrap_or(len - 1);
    let thinnest = (len as f64 * THINNEST).ceil() as usize;
    let bar = |n: usize| if n < thinnest { 0 } else { (n as u32) & !1 };
    (bar(first), bar(len - 1 - last))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../media/tests/fixtures")
            .join(name)
    }

    #[test]
    fn dark_ends_are_bars_and_thin_or_missing_ones_are_not() {
        let mut line = vec![200u8; 100];
        line[..20].fill(10);
        line[87..].fill(16);
        assert_eq!(edges(&line), (20, 12));
        // Odd: made even, so what is left is too.
        line[..21].fill(10);
        assert_eq!(edges(&line), (20, 12));
        // A dark line at the very edge is the picture's own.
        let mut thin = vec![200u8; 1000];
        thin[0] = 0;
        assert_eq!(edges(&thin), (0, 0));
        assert_eq!(edges(&[0; 50]), (0, 0), "all dark: nothing to crop to");
    }

    fn found(picture: (u32, u32), left: u32, top: u32, right: u32, bottom: u32) -> Bars {
        Bars {
            picture,
            left,
            top,
            right,
            bottom,
        }
    }

    #[test]
    fn bars_that_nearly_leave_a_usual_shape_leave_it_exactly() {
        // A 2560×1440 game in a 3440×1440 recording, its edges smeared.
        let game = snapped(found((3440, 1440), 436, 0, 438, 0));
        assert_eq!((game.left, game.right), (440, 440));
        assert_eq!(game.inside(), (2560, 1440));
        // 4:3 in 16:9, letterboxed the other way: 21:9 in 16:9.
        assert_eq!(
            snapped(found((1920, 1080), 238, 0, 242, 0)).inside(),
            (1440, 1080)
        );
        assert_eq!(snapped(found((1920, 1080), 0, 132, 0, 130)).inside(), (1920, 810));
        // Lopsided, or no usual shape near: left as found.
        assert_eq!(snapped(found((3440, 1440), 100, 0, 774, 0)).left, 100);
        assert_eq!(snapped(found((3440, 1440), 300, 0, 300, 0)).left, 300);
    }

    #[test]
    fn a_narrower_game_in_a_wide_recording_is_found() {
        // 86×36 with the middle 64 columns grey and black at the sides
        // (11 each), as 2560 in 3440, small.
        let bars = find(&fixture("pillarbox.mkv"), 0, 0, 0.0, 1.0).unwrap();
        assert_eq!(bars.picture, (86, 36));
        assert_eq!((bars.top, bars.bottom), (0, 0));
        assert!(
            bars.left.abs_diff(10) <= 2 && bars.right.abs_diff(10) <= 2,
            "{bars:?}"
        );
        assert!(bars.inside().0.abs_diff(64) <= 4);
        // A recording without bars.
        let none = find(&fixture("game-3-audio.mkv"), 0, 0, 0.0, 2.0).unwrap();
        assert!(!none.any(), "{none:?}");
    }
}
