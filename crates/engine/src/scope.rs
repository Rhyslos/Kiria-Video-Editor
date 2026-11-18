//! Scopes for the Color page, from the timeline's real picture: the frame
//! the preview put together, drawn small on the graphics card and read
//! back (`Composer::sample`), then counted here on the preview's worker.
//! Each scope is a small grey picture of counts, brighter where more
//! pixels fall, ready to show; with it, one line in plain words on what
//! is clipped.

/// Which scope.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ScopeKind {
    /// Brightness (up) at each place across the picture.
    #[default]
    Waveform,
    /// Red, green and blue each as a waveform, side by side.
    Parade,
    /// Colour: hue around, saturation outwards.
    Vectorscope,
    /// How many pixels at each brightness, red, green and blue.
    Histogram,
}

impl ScopeKind {
    pub const ALL: [ScopeKind; 4] = [
        ScopeKind::Waveform,
        ScopeKind::Parade,
        ScopeKind::Vectorscope,
        ScopeKind::Histogram,
    ];

    pub fn name(self) -> &'static str {
        match self {
            ScopeKind::Waveform => "Waveform",
            ScopeKind::Parade => "RGB parade",
            ScopeKind::Vectorscope => "Vectorscope",
            ScopeKind::Histogram => "Histogram",
        }
    }
}

/// A small picture of a scope: RGBA, row by row from the top.
#[derive(Debug, Clone, PartialEq)]
pub struct ScopeImage {
    pub width: usize,
    pub height: usize,
    pub rgba: Vec<u8>,
}

/// The picture a scope reads: display values (0 to 1) of a small copy of
/// the frame, red, green and blue, row by row.
#[derive(Debug, Clone, PartialEq)]
pub struct Sampled {
    pub width: usize,
    pub height: usize,
    pub rgb: Vec<[f32; 3]>,
}

/// How tall a waveform and the histogram are, in levels.
const LEVELS: usize = 128;

/// How bright a pixel is (BT.709).
fn luma([r, g, b]: [f32; 3]) -> f32 {
    0.2126 * r + 0.7152 * g + 0.0722 * b
}

fn level(value: f32) -> usize {
    ((value.clamp(0.0, 1.0) * (LEVELS - 1) as f32).round() as usize).min(LEVELS - 1)
}

/// Counts as brightness: a count `share` of the busiest shown full.
fn shade(count: u32, busiest: u32) -> u8 {
    if count == 0 || busiest == 0 {
        return 0;
    }
    // The square root shows a few pixels as clearly as many.
    let share = (count as f32 / busiest as f32).sqrt();
    (40.0 + 215.0 * share).round().min(255.0) as u8
}

impl Sampled {
    /// The scope `kind` of this picture.
    pub fn image(&self, kind: ScopeKind) -> ScopeImage {
        match kind {
            ScopeKind::Waveform => self.waveform(),
            ScopeKind::Parade => self.parade(),
            ScopeKind::Vectorscope => self.vectorscope(),
            ScopeKind::Histogram => self.histogram(),
        }
    }

    fn waveform(&self) -> ScopeImage {
        let width = self.width.max(1);
        let mut counts = vec![0u32; width * LEVELS];
        for (i, p) in self.rgb.iter().enumerate() {
            let x = i % width;
            counts[(LEVELS - 1 - level(luma(*p))) * width + x] += 1;
        }
        let busiest = counts.iter().copied().max().unwrap_or(0);
        let rgba = counts
            .iter()
            .flat_map(|c| {
                let v = shade(*c, busiest);
                [v, v, v, 255]
            })
            .collect();
        ScopeImage {
            width,
            height: LEVELS,
            rgba,
        }
    }

    fn parade(&self) -> ScopeImage {
        let part = self.width.max(1);
        let width = part * 3;
        let mut counts = vec![[0u32; 3]; width * LEVELS];
        for (i, p) in self.rgb.iter().enumerate() {
            let x = i % part;
            for (k, value) in p.iter().enumerate() {
                counts[(LEVELS - 1 - level(*value)) * width + k * part + x][k] += 1;
            }
        }
        let busiest = counts.iter().flat_map(|c| c.iter().copied()).max().unwrap_or(0);
        let rgba = counts
            .iter()
            .flat_map(|c| {
                [
                    shade(c[0], busiest),
                    shade(c[1], busiest),
                    shade(c[2], busiest),
                    255,
                ]
            })
            .collect();
        ScopeImage {
            width,
            height: LEVELS,
            rgba,
        }
    }

    fn vectorscope(&self) -> ScopeImage {
        let size = LEVELS;
        let mut counts = vec![0u32; size * size];
        for p in &self.rgb {
            let y = luma(*p);
            // BT.709 colour differences, about -0.5 to 0.5.
            let cb = (p[2] - y) / 1.8556;
            let cr = (p[0] - y) / 1.5748;
            let x = ((0.5 + cb) * (size - 1) as f32)
                .round()
                .clamp(0.0, (size - 1) as f32) as usize;
            let row = ((0.5 - cr) * (size - 1) as f32)
                .round()
                .clamp(0.0, (size - 1) as f32) as usize;
            counts[row * size + x] += 1;
        }
        let busiest = counts.iter().copied().max().unwrap_or(0);
        let rgba = counts
            .iter()
            .flat_map(|c| {
                let v = shade(*c, busiest);
                [v, v, v, 255]
            })
            .collect();
        ScopeImage {
            width: size,
            height: size,
            rgba,
        }
    }

    fn histogram(&self) -> ScopeImage {
        let bins = 128;
        let mut counts = [[0u32; 3]; 128];
        for p in &self.rgb {
            for (k, value) in p.iter().enumerate() {
                counts[level(*value) * bins / LEVELS][k] += 1;
            }
        }
        let busiest = counts
            .iter()
            .flat_map(|c| c.iter().copied())
            .max()
            .unwrap_or(0)
            .max(1);
        let height = LEVELS;
        let mut rgba = vec![0u8; bins * height * 4];
        for (x, c) in counts.iter().enumerate() {
            for (k, count) in c.iter().enumerate() {
                let tall = (*count as f32 / busiest as f32 * height as f32).round() as usize;
                for row in height.saturating_sub(tall)..height {
                    let at = (row * bins + x) * 4;
                    rgba[at + k] = 210;
                    rgba[at + 3] = 255;
                }
            }
        }
        for px in rgba.chunks_mut(4) {
            px[3] = 255;
        }
        ScopeImage {
            width: bins,
            height,
            rgba,
        }
    }

    /// The light (display values undone) around `at` (0 to 1 across and
    /// down), the mean of the pixels within `reach` of it.
    pub fn light_around(&self, at: (f32, f32), reach: usize) -> Option<[f32; 3]> {
        if self.rgb.is_empty() || !(0.0..=1.0).contains(&at.0) || !(0.0..=1.0).contains(&at.1) {
            return None;
        }
        let x = ((at.0 * self.width as f32) as usize).min(self.width - 1);
        let y = ((at.1 * self.height as f32) as usize).min(self.height - 1);
        let mut sum = [0.0f32; 3];
        let mut n = 0.0;
        for row in y.saturating_sub(reach)..(y + reach + 1).min(self.height) {
            for col in x.saturating_sub(reach)..(x + reach + 1).min(self.width) {
                let p = self.rgb[row * self.width + col];
                for k in 0..3 {
                    sum[k] += p[k].powf(2.4);
                }
                n += 1.0;
            }
        }
        Some(sum.map(|s| s / n))
    }

    /// One line on what is clipped: "Nothing is clipped", or how much of
    /// the picture is lost at white or at black.
    pub fn reading(&self) -> String {
        let n = self.rgb.len().max(1) as f32;
        let white = self.rgb.iter().filter(|p| p.iter().any(|v| *v >= 0.995)).count() as f32 / n;
        let black = self.rgb.iter().filter(|p| p.iter().all(|v| *v <= 0.005)).count() as f32 / n;
        let percent = |share: f32| (share * 100.0).round().max(1.0);
        match (white >= 0.005, black >= 0.005) {
            (false, false) => "Nothing is clipped".into(),
            (true, false) => format!("{:.0} % of the picture is clipped at white", percent(white)),
            (false, true) => format!("{:.0} % of the picture is crushed to black", percent(black)),
            (true, true) => format!(
                "{:.0} % clipped at white, {:.0} % crushed to black",
                percent(white),
                percent(black)
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn picture(rgb: [f32; 3]) -> Sampled {
        Sampled {
            width: 4,
            height: 2,
            rgb: vec![rgb; 8],
        }
    }

    #[test]
    fn a_grey_picture_is_one_line_on_the_waveform_and_a_dot_in_the_middle_of_the_vectorscope() {
        let grey = picture([0.5, 0.5, 0.5]);
        let wave = grey.image(ScopeKind::Waveform);
        let lit_rows: Vec<usize> = (0..wave.height)
            .filter(|row| (0..wave.width).any(|x| wave.rgba[(row * wave.width + x) * 4] > 0))
            .collect();
        assert_eq!(lit_rows, vec![LEVELS - 1 - level(0.5)]);
        let vector = grey.image(ScopeKind::Vectorscope);
        let middle = (LEVELS / 2) * LEVELS + LEVELS / 2;
        let lit: Vec<usize> = (0..LEVELS * LEVELS).filter(|i| vector.rgba[i * 4] > 0).collect();
        assert_eq!(lit.len(), 1);
        assert!(
            (lit[0] as i64 - middle as i64).abs() <= LEVELS as i64 + 1,
            "near the middle"
        );
    }

    #[test]
    fn the_reading_says_what_is_clipped() {
        assert_eq!(picture([0.5, 0.5, 0.5]).reading(), "Nothing is clipped");
        assert_eq!(
            picture([1.0, 0.4, 0.4]).reading(),
            "100 % of the picture is clipped at white"
        );
        assert_eq!(
            picture([0.0, 0.0, 0.0]).reading(),
            "100 % of the picture is crushed to black"
        );
    }

    #[test]
    fn the_light_around_a_point_is_the_mean_of_its_pixels() {
        let mut p = picture([1.0, 1.0, 1.0]);
        p.rgb[0] = [0.0, 0.0, 0.0];
        let corner = p.light_around((0.0, 0.0), 0).expect("inside");
        assert_eq!(corner, [0.0, 0.0, 0.0]);
        let middle = p.light_around((0.6, 0.6), 0).expect("inside");
        assert_eq!(middle, [1.0, 1.0, 1.0]);
        assert_eq!(p.light_around((1.5, 0.5), 1), None);
    }

    #[test]
    fn every_scope_has_a_picture_of_its_size() {
        let p = picture([0.2, 0.6, 0.9]);
        for kind in ScopeKind::ALL {
            let image = p.image(kind);
            assert_eq!(
                image.rgba.len(),
                image.width * image.height * 4,
                "{}",
                kind.name()
            );
        }
    }
}
