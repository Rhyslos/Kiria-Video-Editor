//! Pictures as the decoder made them: YUV planes, not yet turned into RGB.
//!
//! The viewer uploads these planes and a shader converts them while drawing,
//! so the CPU never touches a pixel twice. A [`YuvFrame`] carries everything
//! that conversion needs: the planes without row padding, how they are laid
//! out, which colour matrix and range the file uses, and the turn that shows
//! the picture upright. When the viewer is small the planes are halved on
//! the way out (by averaging: under a millisecond for a 1440p frame), so less
//! is uploaded and the GPU never shrinks a picture by much in one go.
//!
//! Only 8-bit 4:2:0 is handled here (what game recordings are); every other
//! format goes through the RGBA path in [`crate::decode`].

use crate::decode::{Frame, is_high_definition, turn};
use ffmpeg_sys_next as ff;
use std::sync::{Arc, Mutex, Weak};

/// How the two colour channels are stored after the luma plane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layout {
    /// One plane with U and V side by side in each pair of bytes (NV12): what
    /// GPU decoders copy back.
    Interleaved,
    /// All of U, then all of V: what CPU decoders give.
    Planar,
}

/// Which weights turn luma and colour difference into red, green and blue.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Matrix {
    /// Standard definition.
    Bt601,
    /// High definition.
    Bt709,
}

/// Which stored values mean black and white.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Range {
    /// Luma 16 to 235, colour 16 to 240: broadcast video.
    Limited,
    /// 0 to 255: most game recorders.
    Full,
}

/// How a file's stored values map to colours.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Colour {
    pub matrix: Matrix,
    pub range: Range,
}

impl Colour {
    /// The colour description of a decoded picture, from what the file says
    /// (`full_range_format`: its pixels came in one of the old full-range
    /// formats). A file that doesn't say which matrix it uses gets the usual
    /// guess by size. `None` for matrices the shader doesn't know (wide
    /// gamut and the rare ones): those pictures take the RGBA path.
    pub(crate) fn of(
        space: ff::AVColorSpace,
        range: ff::AVColorRange,
        full_range_format: bool,
        width: u32,
        height: u32,
    ) -> Option<Colour> {
        use ff::AVColorSpace as S;
        let matrix = match space {
            S::AVCOL_SPC_BT709 => Matrix::Bt709,
            S::AVCOL_SPC_BT470BG | S::AVCOL_SPC_SMPTE170M => Matrix::Bt601,
            S::AVCOL_SPC_UNSPECIFIED if is_high_definition(width, height) => Matrix::Bt709,
            S::AVCOL_SPC_UNSPECIFIED => Matrix::Bt601,
            _ => return None,
        };
        // The "J" pixel formats are the old way of saying full range.
        let full = range == ff::AVColorRange::AVCOL_RANGE_JPEG || full_range_format;
        Some(Colour {
            matrix,
            range: if full { Range::Full } else { Range::Limited },
        })
    }

    /// The conversion as three rows, for red, green and blue: each is the
    /// weights of Y, U and V (as stored, scaled to 0–1) and a constant, and
    /// gives that colour in 0–1 (before clamping). The shader and
    /// [`YuvFrame::to_rgba`] both use exactly these numbers.
    pub fn rgb_rows(self) -> [[f32; 4]; 3] {
        // Luma weights of red and blue; green's follows.
        let (kr, kb) = match self.matrix {
            Matrix::Bt601 => (0.299, 0.114),
            Matrix::Bt709 => (0.2126, 0.0722),
        };
        let kg = 1.0 - kr - kb;
        // Stored values to luma 0–1 and colour difference ±0.5.
        let (y_scale, y_zero, c_scale): (f64, f64, f64) = match self.range {
            Range::Limited => (255.0 / 219.0, 16.0 / 255.0, 255.0 / 224.0),
            Range::Full => (1.0, 0.0, 1.0),
        };
        let c_zero = 128.0 / 255.0;
        let row = |u: f64, v: f64| {
            let (u, v) = (u * c_scale, v * c_scale);
            [
                y_scale as f32,
                u as f32,
                v as f32,
                (-y_zero * y_scale - c_zero * (u + v)) as f32,
            ]
        };
        [
            row(0.0, 2.0 * (1.0 - kr)),
            row(-2.0 * kb * (1.0 - kb) / kg, -2.0 * kr * (1.0 - kr) / kg),
            row(2.0 * (1.0 - kb), 0.0),
        ]
    }
}

/// Plane buffers a decoder gets back when the pictures made from them are
/// dropped, so playing doesn't allocate megabytes for every frame.
#[derive(Debug, Clone, Default)]
pub(crate) struct Pool {
    spare: Arc<Mutex<Vec<Vec<u8>>>>,
}

/// More spare buffers than this are freed instead of kept.
const SPARE_BUFFERS: usize = 3;

impl Pool {
    /// An empty buffer, reused when one is spare.
    pub(crate) fn take(&self) -> Planes {
        let mut bytes = self
            .spare
            .lock()
            .ok()
            .and_then(|mut spare| spare.pop())
            .unwrap_or_default();
        bytes.clear();
        Planes {
            bytes,
            home: Arc::downgrade(&self.spare),
        }
    }
}

/// The bytes of a picture's planes, one after another without padding.
/// Dropping it hands the memory back to the decoder it came from, if that
/// decoder is still open.
pub struct Planes {
    bytes: Vec<u8>,
    home: Weak<Mutex<Vec<Vec<u8>>>>,
}

impl std::fmt::Debug for Planes {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Planes({} bytes)", self.bytes.len())
    }
}

impl AsRef<[u8]> for Planes {
    fn as_ref(&self) -> &[u8] {
        &self.bytes
    }
}

impl Drop for Planes {
    fn drop(&mut self) {
        if let Some(home) = self.home.upgrade()
            && let Ok(mut spare) = home.lock()
            && spare.len() < SPARE_BUFFERS
        {
            spare.push(std::mem::take(&mut self.bytes));
        }
    }
}

/// A decoded picture as YUV planes: 8 bits a sample, colour at half size
/// both ways (4:2:0). Stored as the file has it; `rotation` says how to turn it.
pub struct YuvFrame {
    /// The luma plane's size. The colour planes are half of it, rounded up.
    pub width: u32,
    pub height: u32,
    pub layout: Layout,
    pub colour: Colour,
    /// The clockwise turn (0, 90, 180 or 270) that shows it upright.
    pub rotation: u16,
    /// When it starts, in seconds from the start of the file.
    pub time: f64,
    planes: Planes,
}

impl std::fmt::Debug for YuvFrame {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "YuvFrame({}×{} {:?} {:?} at {:.3} s)",
            self.width, self.height, self.layout, self.colour, self.time
        )
    }
}

/// Half of a luma size, rounded up: the size of a colour plane.
pub fn chroma_size(width: u32, height: u32) -> (u32, u32) {
    (width.div_ceil(2), height.div_ceil(2))
}

impl YuvFrame {
    /// The size of each colour plane (of the one plane when interleaved,
    /// counted in U and V pairs).
    pub fn chroma_size(&self) -> (u32, u32) {
        chroma_size(self.width, self.height)
    }

    /// The size on screen, after turning upright.
    pub fn upright_size(&self) -> (u32, u32) {
        if self.rotation % 180 == 90 {
            (self.height, self.width)
        } else {
            (self.width, self.height)
        }
    }

    fn luma_len(&self) -> usize {
        self.width as usize * self.height as usize
    }

    /// The luma plane, `width` bytes a row.
    pub fn luma(&self) -> &[u8] {
        &self.planes.bytes[..self.luma_len()]
    }

    /// The colour samples: interleaved U and V pairs, or all of U then all
    /// of V, by `layout`. Twice a colour plane's size in bytes either way.
    pub fn chroma(&self) -> &[u8] {
        &self.planes.bytes[self.luma_len()..]
    }

    /// The planes' bytes (luma, then colour), to upload without copying.
    pub fn into_planes(self) -> Planes {
        self.planes
    }

    /// The same conversion the shader does, on the CPU, turned upright. Each
    /// pixel takes the colour sample it sits in (no smoothing between them).
    /// For checks and for anything that needs pixels without a GPU.
    pub fn to_rgba(&self) -> Frame {
        let rows = self.colour.rgb_rows();
        let (w, h) = (self.width as usize, self.height as usize);
        let (cw, ch) = self.chroma_size();
        let (cw, ch) = (cw as usize, ch as usize);
        let (luma, chroma) = (self.luma(), self.chroma());
        let mut rgba = Vec::with_capacity(w * h * 4);
        for y in 0..h {
            for x in 0..w {
                let at = (y / 2) * cw + x / 2;
                let (u, v) = match self.layout {
                    Layout::Interleaved => (chroma[at * 2], chroma[at * 2 + 1]),
                    Layout::Planar => (chroma[at], chroma[cw * ch + at]),
                };
                let yuv = [luma[y * w + x], u, v].map(|s| f32::from(s) / 255.0);
                for row in rows {
                    let c = row[0] * yuv[0] + row[1] * yuv[1] + row[2] * yuv[2] + row[3];
                    rgba.push((c * 255.0).round().clamp(0.0, 255.0) as u8);
                }
                rgba.push(255);
            }
        }
        let (width, height, rgba) = turn(rgba, self.width, self.height, self.rotation);
        Frame {
            width,
            height,
            rgba,
            time: self.time,
        }
    }
}

/// One plane inside a decoded frame: rows of `row_bytes` bytes, `stride`
/// bytes apart (decoders pad rows).
#[derive(Debug, Clone, Copy)]
pub(crate) struct PlaneRef<'a> {
    /// From the first row's first byte to the last row's last.
    pub bytes: &'a [u8],
    pub stride: usize,
    pub row_bytes: usize,
    pub rows: usize,
}

impl PlaneRef<'_> {
    fn row(&self, y: usize) -> &[u8] {
        &self.bytes[y * self.stride..y * self.stride + self.row_bytes]
    }

    /// Appends the plane to `out` without the padding.
    fn copy_to(&self, out: &mut Vec<u8>) {
        if self.stride == self.row_bytes {
            out.extend_from_slice(self.bytes);
        } else {
            for y in 0..self.rows {
                out.extend_from_slice(self.row(y));
            }
        }
    }

    /// Appends the plane to `out` at half its size both ways (rounded up),
    /// each sample the average of the four it replaces. `N` is 2 for
    /// interleaved colour, so U only mixes with U and V with V.
    fn halve_to<const N: usize>(&self, out: &mut Vec<u8>) {
        let across = self.row_bytes / N;
        let mean = |a: u8, b: u8, c: u8, d: u8| {
            ((u16::from(a) + u16::from(b) + u16::from(c) + u16::from(d) + 2) / 4) as u8
        };
        for top in (0..self.rows).step_by(2) {
            // The last row of an odd height stands for the missing one too.
            let (upper, lower) = (self.row(top), self.row((top + 1).min(self.rows - 1)));
            let start = out.len();
            out.resize(start + across.div_ceil(2) * N, 0);
            let halved = &mut out[start..];
            let pairs = upper.chunks_exact(2 * N).zip(lower.chunks_exact(2 * N));
            for (to, (a, b)) in halved.chunks_exact_mut(N).zip(pairs) {
                for c in 0..N {
                    to[c] = mean(a[c], a[c + N], b[c], b[c + N]);
                }
            }
            // And the last column of an odd width.
            if across % 2 == 1 {
                let last = (across - 1) * N;
                for c in 0..N {
                    halved[last / 2 + c] =
                        mean(upper[last + c], upper[last + c], lower[last + c], lower[last + c]);
                }
            }
        }
    }
}

/// How much smaller than the file a picture may be made (halving each step)
/// while still filling a `max_width × max_height` box as sharply as the box
/// can show: the largest of 1, 2, 4 and 8 that leaves the upright picture no
/// smaller than its fit in the box.
pub(crate) fn step_for(upright: (u32, u32), max_width: u32, max_height: u32) -> usize {
    let (max_width, max_height) = (max_width.max(1), max_height.max(1));
    let mut step = 1;
    // The fit touches the box on one side at least: halve while that side
    // still has the pixels.
    while step < 8 && (upright.0 >= step * 2 * max_width || upright.1 >= step * 2 * max_height) {
        step *= 2;
    }
    step as usize
}

/// Everything about a decoded frame's planes, borrowed from the decoder.
pub(crate) struct Source<'a> {
    pub luma: PlaneRef<'a>,
    /// One interleaved plane, or U and V.
    pub chroma: [Option<PlaneRef<'a>>; 2],
    pub colour: Colour,
    pub rotation: u16,
    pub time: f64,
}

impl Source<'_> {
    /// Copies the planes out once, into a buffer from `pool`, at 1/`step`
    /// of their size (`step` is 1, 2, 4 or 8; sizes round up).
    pub(crate) fn frame(&self, step: usize, pool: &Pool) -> YuvFrame {
        let interleaved = self.chroma[1].is_none();
        let mut planes = pool.take();
        let (mut width, mut height) = (self.luma.row_bytes, self.luma.rows);
        if step == 1 {
            let (cw, ch) = chroma_size(width as u32, height as u32);
            planes
                .bytes
                .reserve(width * height + 2 * cw as usize * ch as usize);
            for plane in std::iter::once(&self.luma).chain(self.chroma.iter().flatten()) {
                plane.copy_to(&mut planes.bytes);
            }
        } else {
            // Halve from the decoder's frame, then halve that again for a
            // quarter and an eighth: each pass has a quarter of the work of
            // the one before.
            Self::halve(self.luma, self.chroma, &mut planes.bytes);
            (width, height) = (width.div_ceil(2), height.div_ceil(2));
            let mut done = 2;
            while done < step {
                let mut smaller = pool.take();
                let (cw, ch) = chroma_size(width as u32, height as u32);
                let (cw, ch) = (cw as usize, ch as usize);
                let (luma, chroma) = planes.bytes.split_at(width * height);
                let plane = |bytes, row_bytes, rows| PlaneRef {
                    bytes,
                    stride: row_bytes,
                    row_bytes,
                    rows,
                };
                let chroma = if interleaved {
                    [Some(plane(chroma, cw * 2, ch)), None]
                } else {
                    let (u, v) = chroma.split_at(cw * ch);
                    [Some(plane(u, cw, ch)), Some(plane(v, cw, ch))]
                };
                Self::halve(plane(luma, width, height), chroma, &mut smaller.bytes);
                planes = smaller;
                (width, height) = (width.div_ceil(2), height.div_ceil(2));
                done *= 2;
            }
        }
        YuvFrame {
            width: width as u32,
            height: height as u32,
            layout: if interleaved {
                Layout::Interleaved
            } else {
                Layout::Planar
            },
            colour: self.colour,
            rotation: self.rotation,
            time: self.time,
            planes,
        }
    }

    /// Appends all planes to `out` at half size.
    fn halve(luma: PlaneRef<'_>, chroma: [Option<PlaneRef<'_>>; 2], out: &mut Vec<u8>) {
        luma.halve_to::<1>(out);
        match chroma {
            [Some(both), None] => both.halve_to::<2>(out),
            planes => {
                for plane in planes.iter().flatten() {
                    plane.halve_to::<1>(out);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIMITED_709: Colour = Colour {
        matrix: Matrix::Bt709,
        range: Range::Limited,
    };

    fn rgb(colour: Colour, yuv: [u8; 3]) -> [u8; 3] {
        let yuv = yuv.map(|s| f32::from(s) / 255.0);
        colour.rgb_rows().map(|r| {
            let c = r[0] * yuv[0] + r[1] * yuv[1] + r[2] * yuv[2] + r[3];
            (c * 255.0).round().clamp(0.0, 255.0) as u8
        })
    }

    /// Stored values are whole numbers, so a colour comes back within one step.
    #[track_caller]
    fn assert_near(got: [u8; 3], want: [u8; 3]) {
        let close = got.iter().zip(want).all(|(g, w)| g.abs_diff(w) <= 1);
        assert!(close, "got {got:?}, wanted {want:?}");
    }

    #[test]
    fn black_white_and_primaries_come_out_right() {
        assert_eq!(rgb(LIMITED_709, [16, 128, 128]), [0, 0, 0]);
        assert_eq!(rgb(LIMITED_709, [235, 128, 128]), [255, 255, 255]);
        assert_eq!(rgb(LIMITED_709, [126, 128, 128]), [128, 128, 128]);
        // Pure red, green and blue as BT.709 limited range stores them.
        assert_near(rgb(LIMITED_709, [63, 102, 240]), [255, 0, 0]);
        assert_near(rgb(LIMITED_709, [173, 42, 26]), [0, 255, 0]);
        assert_near(rgb(LIMITED_709, [32, 240, 118]), [0, 0, 255]);
        // And as BT.601 stores them: the same bytes mean other colours.
        let sd = Colour {
            matrix: Matrix::Bt601,
            range: Range::Limited,
        };
        assert_near(rgb(sd, [81, 90, 240]), [255, 0, 0]);
        assert_near(rgb(sd, [145, 54, 34]), [0, 255, 0]);
        assert_near(rgb(sd, [41, 240, 110]), [0, 0, 255]);
        assert_eq!(
            rgb(LIMITED_709, [81, 90, 240]),
            [255, 24, 0],
            "red read as HD turns orange"
        );
    }

    #[test]
    fn full_range_uses_every_value() {
        let full = Colour {
            matrix: Matrix::Bt709,
            range: Range::Full,
        };
        assert_eq!(rgb(full, [0, 128, 128]), [0, 0, 0]);
        assert_eq!(rgb(full, [255, 128, 128]), [255, 255, 255]);
        assert_eq!(rgb(full, [16, 128, 128]), [16, 16, 16], "not stretched");
        assert_near(rgb(full, [54, 99, 255]), [255, 0, 0]);
        assert_near(rgb(full, [182, 30, 12]), [0, 255, 0]);
        let sd = Colour {
            matrix: Matrix::Bt601,
            range: Range::Full,
        };
        assert_near(rgb(sd, [29, 255, 107]), [0, 0, 255]);
    }

    #[test]
    fn the_matrix_follows_the_file_else_the_size() {
        use ff::AVColorRange::*;
        use ff::AVColorSpace::*;
        let hd = Colour::of(AVCOL_SPC_BT709, AVCOL_RANGE_MPEG, false, 640, 360);
        assert_eq!(hd, Some(LIMITED_709), "the file's word beats the size");
        for sd in [AVCOL_SPC_BT470BG, AVCOL_SPC_SMPTE170M] {
            let c = Colour::of(sd, AVCOL_RANGE_UNSPECIFIED, false, 1920, 1080);
            assert_eq!(c.map(|c| c.matrix), Some(Matrix::Bt601));
        }
        // Unsaid: by size.
        let guess =
            |w, h| Colour::of(AVCOL_SPC_UNSPECIFIED, AVCOL_RANGE_UNSPECIFIED, false, w, h).map(|c| c.matrix);
        assert_eq!(guess(1920, 1080), Some(Matrix::Bt709));
        assert_eq!(guess(1280, 720), Some(Matrix::Bt709));
        assert_eq!(guess(1280, 544), Some(Matrix::Bt709), "wide but not tall");
        assert_eq!(guess(720, 576), Some(Matrix::Bt601));
        assert_eq!(guess(640, 480), Some(Matrix::Bt601));
        // Wide gamut isn't converted here.
        let wide = Colour::of(AVCOL_SPC_BT2020_NCL, AVCOL_RANGE_MPEG, false, 3840, 2160);
        assert_eq!(wide, None);
    }

    #[test]
    fn full_range_is_told_by_the_tag_or_the_old_format_name() {
        use ff::AVColorRange::*;
        let range = |range, old_format| {
            Colour::of(ff::AVColorSpace::AVCOL_SPC_BT709, range, old_format, 16, 16).map(|c| c.range)
        };
        assert_eq!(range(AVCOL_RANGE_MPEG, false), Some(Range::Limited));
        assert_eq!(range(AVCOL_RANGE_UNSPECIFIED, false), Some(Range::Limited));
        assert_eq!(range(AVCOL_RANGE_JPEG, false), Some(Range::Full));
        assert_eq!(range(AVCOL_RANGE_UNSPECIFIED, true), Some(Range::Full));
    }

    /// A plane of `w × h` whose rows are padded to `stride` with 255s.
    fn padded(w: usize, h: usize, stride: usize, value: impl Fn(usize, usize) -> u8) -> Vec<u8> {
        let mut bytes = vec![255; stride * (h - 1) + w];
        for y in 0..h {
            for x in 0..w {
                bytes[y * stride + x] = value(x, y);
            }
        }
        bytes
    }

    fn plane(bytes: &[u8], w: usize, h: usize, stride: usize) -> PlaneRef<'_> {
        PlaneRef {
            bytes,
            stride,
            row_bytes: w,
            rows: h,
        }
    }

    #[test]
    fn copying_leaves_the_row_padding_behind() {
        let bytes = padded(5, 3, 8, |x, y| (y * 10 + x) as u8);
        let mut out = Vec::new();
        plane(&bytes, 5, 3, 8).copy_to(&mut out);
        assert_eq!(out, [0, 1, 2, 3, 4, 10, 11, 12, 13, 14, 20, 21, 22, 23, 24]);
        // Without padding it is one straight copy.
        let tight = padded(5, 3, 5, |x, y| (y * 10 + x) as u8);
        let mut same = Vec::new();
        plane(&tight, 5, 3, 5).copy_to(&mut same);
        assert_eq!(same, out);
    }

    #[test]
    fn odd_sizes_round_the_colour_planes_up() {
        assert_eq!(chroma_size(1920, 1080), (960, 540));
        assert_eq!(chroma_size(1365, 767), (683, 384));
        assert_eq!(chroma_size(1, 1), (1, 1));
        // A 5×3 picture: 15 luma bytes, then 3×2 of U and of V.
        let y = padded(5, 3, 8, |_, _| 100);
        let u = padded(3, 2, 4, |_, _| 110);
        let v = padded(3, 2, 4, |_, _| 120);
        let source = Source {
            luma: plane(&y, 5, 3, 8),
            chroma: [Some(plane(&u, 3, 2, 4)), Some(plane(&v, 3, 2, 4))],
            colour: LIMITED_709,
            rotation: 90,
            time: 0.0,
        };
        let frame = source.frame(1, &Pool::default());
        assert_eq!((frame.width, frame.height), (5, 3));
        assert_eq!(frame.chroma_size(), (3, 2));
        assert_eq!(frame.upright_size(), (3, 5), "turned on its side");
        assert_eq!(frame.layout, Layout::Planar);
        assert_eq!(frame.luma(), [100; 15]);
        assert_eq!(frame.chroma(), [[110; 6], [120; 6]].concat());
        assert_eq!(frame.to_rgba().rgba.len(), 5 * 3 * 4);
    }

    #[test]
    fn halving_averages_fours_and_keeps_u_from_v() {
        // 6×4 luma: left half 10, right half 30, with one bright pixel.
        let mut y = padded(6, 4, 9, |x, _| if x < 3 { 10 } else { 30 });
        y[0] = 50;
        let mut out = Vec::new();
        plane(&y, 6, 4, 9).halve_to::<1>(&mut out);
        assert_eq!(out, [20, 20, 30, 10, 20, 30], "3×2, the mixed column in between");
        // Interleaved colour, 3 pairs wide: U rises 100, 104, 108 and V is 200.
        let uv = padded(6, 2, 8, |x, _| if x % 2 == 0 { 100 + 2 * x as u8 } else { 200 });
        let mut out = Vec::new();
        plane(&uv, 6, 2, 8).halve_to::<2>(&mut out);
        assert_eq!(
            out,
            [102, 200, 108, 200],
            "2 pairs: the last one from a lone column"
        );
        // Odd both ways: the last column and row stand alone.
        let odd = padded(3, 3, 5, |x, y| if x == 2 || y == 2 { 80 } else { 40 });
        let mut out = Vec::new();
        plane(&odd, 3, 3, 5).halve_to::<1>(&mut out);
        assert_eq!(out, [40, 80, 80, 80]);
    }

    #[test]
    fn shrunk_frames_keep_their_shape() {
        let y = padded(8, 6, 8, |_, _| 60);
        let uv = padded(8, 3, 8, |x, _| if x % 2 == 0 { 90 } else { 170 });
        let source = Source {
            luma: plane(&y, 8, 6, 8),
            chroma: [Some(plane(&uv, 8, 3, 8)), None],
            colour: LIMITED_709,
            rotation: 0,
            time: 1.5,
        };
        let pool = Pool::default();
        let half = source.frame(2, &pool);
        assert_eq!((half.width, half.height), (4, 3));
        assert_eq!(half.layout, Layout::Interleaved);
        assert_eq!(half.chroma_size(), (2, 2));
        assert_eq!(half.luma(), [60; 12]);
        assert_eq!(half.chroma(), [90, 170, 90, 170, 90, 170, 90, 170]);
        assert_eq!(half.time, 1.5);
        // A quarter: halved twice. 2×2 luma (6 rows make 3, then 2), one pair of colour.
        let quarter = source.frame(4, &pool);
        assert_eq!((quarter.width, quarter.height), (2, 2));
        assert_eq!(quarter.luma(), [60; 4]);
        assert_eq!(quarter.chroma(), [90, 170]);
        // Planar, to an eighth: 16×16 becomes 2×2 with one sample of U and of V.
        let (y, u, v) = (
            padded(16, 16, 20, |_, _| 60),
            padded(8, 8, 12, |_, _| 90),
            padded(8, 8, 12, |_, _| 170),
        );
        let source = Source {
            luma: plane(&y, 16, 16, 20),
            chroma: [Some(plane(&u, 8, 8, 12)), Some(plane(&v, 8, 8, 12))],
            ..source
        };
        let eighth = source.frame(8, &pool);
        assert_eq!((eighth.width, eighth.height), (2, 2));
        assert_eq!(eighth.layout, Layout::Planar);
        assert_eq!(eighth.luma(), [60; 4]);
        assert_eq!(eighth.chroma(), [90, 170]);
    }

    #[test]
    fn the_step_never_goes_below_what_the_box_can_show() {
        let qhd = (2560, 1440);
        assert_eq!(step_for(qhd, 2560, 1440), 1);
        assert_eq!(step_for(qhd, 4000, 4000), 1, "never enlarged");
        assert_eq!(step_for(qhd, 1300, 800), 1, "half would be too soft");
        assert_eq!(step_for(qhd, 1280, 720), 2);
        assert_eq!(step_for(qhd, 800, 450), 2);
        assert_eq!(step_for(qhd, 640, 360), 4);
        assert_eq!(step_for(qhd, 10, 10), 8, "no further than an eighth");
        // A box of another shape: the picture's fit in it decides.
        assert_eq!(step_for((320, 180), 160, 160), 2);
        assert_eq!(step_for((180, 320), 1000, 1000), 1);
    }

    #[test]
    fn dropped_planes_go_back_to_their_pool() {
        let pool = Pool::default();
        let mut first = pool.take();
        first.bytes.extend_from_slice(&[1, 2, 3]);
        let kept = first.bytes.as_ptr();
        drop(first);
        let again = pool.take();
        assert!(again.bytes.is_empty(), "handed out empty");
        assert_eq!(again.bytes.as_ptr(), kept, "the same memory");
        // With the decoder gone the memory is simply freed.
        let orphan = pool.take();
        drop(pool);
        drop(orphan);
        drop(again);
    }
}
