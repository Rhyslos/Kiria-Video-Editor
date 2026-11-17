//! A title's words as a picture: laid out in the frame by its style, and
//! drawn on the processor into just the part of the frame it covers, at
//! the size that part takes on screen. The font is the interface's own
//! (shipped with the app), drawn a little heavier so it reads over video;
//! preview and export draw it the same way.

use project::{Title, TitleStyle};
use render::Part;

use ab_glyph::{Font, FontRef, PxScale, ScaleFont, point};

/// A title as a layer draws it: its words, the timeline's frame size it is
/// laid out in, and the part of that frame drawn (0 to 1 of it).
#[derive(Debug, Clone, PartialEq)]
pub struct TitleLayer {
    pub title: Title,
    pub frame: (u32, u32),
    pub region: Part,
}

/// The font titles are drawn in.
fn font() -> Option<FontRef<'static>> {
    FontRef::try_from_slice(epaint_default_fonts::UBUNTU_LIGHT).ok()
}

/// One line placed in the frame, in frame pixels.
#[derive(Debug, Clone, PartialEq)]
struct Line {
    text: String,
    /// Where its first letter starts, and its baseline.
    x: f32,
    baseline: f32,
    /// How tall its letters are.
    em: f32,
    /// How bright, 0 to 1 (the lines under a lower third's name are dimmer).
    strength: f32,
}

/// Where everything of a title goes in a frame of `frame` pixels.
#[derive(Debug, Clone, PartialEq)]
struct Layout {
    lines: Vec<Line>,
    /// The dark band or card behind it: left, top, right, bottom, and how
    /// dark it is (its alpha).
    backing: Option<([f32; 4], f32)>,
    /// What it covers: left, top, right, bottom.
    covers: [f32; 4],
}

/// How far apart lines are, in letter heights.
const LINE_SPACING: f32 = 1.25;

/// The letters' width added on each side to draw them heavier, in letter
/// heights.
const WEIGHT: f32 = 0.022;

fn layout(title: &Title, frame: (u32, u32)) -> Option<Layout> {
    let font = font()?;
    let (w, h) = (frame.0.max(1) as f32, frame.1.max(1) as f32);
    let em = (title.size as f32 / 1_000_000.0) * h;
    let lower_third = title.style == TitleStyle::LowerThird;
    // Each line's letter height and strength: under a lower third's name,
    // smaller and dimmer.
    let texts: Vec<&str> = title.text.lines().collect();
    let sized: Vec<(&str, f32, f32)> = texts
        .iter()
        .enumerate()
        .map(|(i, text)| match (lower_third, i) {
            (true, 1..) => (*text, em * 0.7, 0.85),
            _ => (*text, em, 1.0),
        })
        .collect();
    let width_of = |text: &str, em: f32| {
        let scaled = font.as_scaled(PxScale::from(em));
        let mut width = 0.0;
        let mut previous = None;
        for c in text.chars() {
            let id = scaled.glyph_id(c);
            if let Some(previous) = previous {
                width += scaled.kern(previous, id);
            }
            width += scaled.h_advance(id);
            previous = Some(id);
        }
        width
    };
    let widths: Vec<f32> = sized.iter().map(|(text, em, _)| width_of(text, *em)).collect();
    let block_width = widths.iter().copied().fold(0.0, f32::max);
    let block_height: f32 = sized.iter().map(|(_, em, _)| em * LINE_SPACING).sum();
    let (left, top) = match title.style {
        TitleStyle::LowerThird => (0.06 * w, 0.86 * h - block_height),
        TitleStyle::Centred | TitleStyle::EndCard => ((w - block_width) / 2.0, (h - block_height) / 2.0),
    };
    let mut lines = Vec::with_capacity(sized.len());
    let mut y = top;
    for ((text, line_em, strength), width) in sized.into_iter().zip(&widths) {
        let scaled = font.as_scaled(PxScale::from(line_em));
        let gap = line_em * (LINE_SPACING - 1.0) / 2.0;
        let x = match title.style {
            TitleStyle::LowerThird => left,
            _ => (w - width) / 2.0,
        };
        lines.push(Line {
            text: text.to_owned(),
            x,
            baseline: y + gap + scaled.ascent(),
            em: line_em,
            strength,
        });
        y += line_em * LINE_SPACING;
    }
    let block = [left, top, left + block_width, top + block_height];
    let (pad_x, pad_y) = (0.5 * em, 0.25 * em);
    let padded = [
        block[0] - pad_x,
        block[1] - pad_y,
        block[2] + pad_x,
        block[3] + pad_y,
    ];
    let backing = match (title.backing, title.style) {
        (false, _) => None,
        (true, TitleStyle::EndCard) => Some(([0.0, 0.0, w, h], 1.0)),
        (true, _) => Some((padded, 0.6)),
    };
    let covers = backing.map_or(padded, |(rect, _)| rect);
    Some(Layout {
        lines,
        backing,
        covers,
    })
}

/// The part of a frame of `frame` pixels a title covers, 0 to 1 of it:
/// `None` when it covers nothing (no words and no backing).
pub fn bounds(title: &Title, frame: (u32, u32)) -> Option<Part> {
    let laid = layout(title, frame)?;
    if laid.backing.is_none() && laid.lines.iter().all(|l| l.text.trim().is_empty()) {
        return None;
    }
    let (w, h) = (frame.0.max(1) as f32, frame.1.max(1) as f32);
    let [l, t, r, b] = laid.covers;
    let (l, t) = ((l / w).clamp(0.0, 1.0), (t / h).clamp(0.0, 1.0));
    let (r, b) = ((r / w).clamp(0.0, 1.0), (b / h).clamp(0.0, 1.0));
    (r > l && b > t).then_some(Part {
        left: l,
        top: t,
        width: r - l,
        height: b - t,
    })
}

/// The part of the frame between two parts of it, if any.
pub fn overlap(a: Part, b: Part) -> Option<Part> {
    let left = a.left.max(b.left);
    let top = a.top.max(b.top);
    let right = (a.left + a.width).min(b.left + b.width);
    let bottom = (a.top + a.height).min(b.top + b.height);
    (right > left && bottom > top).then_some(Part {
        left,
        top,
        width: right - left,
        height: bottom - top,
    })
}

/// The title's region drawn at `size` pixels, in straight (not
/// premultiplied) RGBA, as decoded pictures come.
pub fn draw(layer: &TitleLayer, size: (u32, u32)) -> media::Frame {
    let (pw, ph) = (size.0.clamp(1, 8192), size.1.clamp(1, 8192));
    let mut frame = media::Frame {
        width: pw,
        height: ph,
        rgba: vec![0; pw as usize * ph as usize * 4],
        time: 0.0,
    };
    let (Some(laid), Some(font)) = (layout(&layer.title, layer.frame), font()) else {
        return frame;
    };
    let (w, h) = (layer.frame.0.max(1) as f32, layer.frame.1.max(1) as f32);
    let region = layer.region;
    // From frame pixels to this picture's.
    let sx = pw as f32 / (region.width * w).max(1e-3);
    let sy = ph as f32 / (region.height * h).max(1e-3);
    let to_x = |x: f32| (x - region.left * w) * sx;
    let to_y = |y: f32| (y - region.top * h) * sy;
    let (cols, rows) = (pw as usize, ph as usize);
    // How much of each pixel the letters cover, and how bright they are there.
    let mut cover = vec![0.0f32; cols * rows];
    let mut bright = vec![0.0f32; cols * rows];
    for line in &laid.lines {
        let mut mask = vec![0.0f32; cols * rows];
        let scaled = font.as_scaled(PxScale::from(line.em * sy));
        let mut x = to_x(line.x);
        let baseline = to_y(line.baseline);
        let mut previous = None;
        for c in line.text.chars() {
            let id = scaled.glyph_id(c);
            if let Some(previous) = previous {
                x += scaled.kern(previous, id);
            }
            let glyph = id.with_scale_and_position(PxScale::from(line.em * sy), point(x, baseline));
            x += scaled.h_advance(id);
            previous = Some(id);
            let Some(outline) = font.outline_glyph(glyph) else {
                continue;
            };
            let edge = outline.px_bounds();
            outline.draw(|gx, gy, c| {
                let (px, py) = (edge.min.x as i64 + gx as i64, edge.min.y as i64 + gy as i64);
                if px >= 0 && py >= 0 && (px as usize) < cols && (py as usize) < rows {
                    let cell = &mut mask[py as usize * cols + px as usize];
                    *cell = cell.max(c);
                }
            });
        }
        let reach = (WEIGHT * line.em * sy).round() as usize;
        heavier(&mut mask, cols, rows, reach);
        for (i, m) in mask.into_iter().enumerate() {
            if m > cover[i] {
                cover[i] = m;
                bright[i] = line.strength;
            }
        }
    }
    let backing = laid.backing.map(|(rect, alpha)| {
        let [l, t, r, b] = rect;
        ((to_x(l), to_y(t), to_x(r), to_y(b)), alpha)
    });
    let text = layer.title.colour.map(f32::from);
    let dark = [16.0f32, 16.0, 18.0];
    for row in 0..rows {
        for col in 0..cols {
            let i = row * cols + col;
            let behind = backing.map_or(0.0, |((l, t, r, b), alpha)| {
                // Its edges cover pixels by how much of them they take.
                let across = ((col as f32 + 1.0).min(r) - (col as f32).max(l)).clamp(0.0, 1.0);
                let down = ((row as f32 + 1.0).min(b) - (row as f32).max(t)).clamp(0.0, 1.0);
                across * down * alpha
            });
            let letters = cover[i].clamp(0.0, 1.0);
            let alpha = letters + behind * (1.0 - letters);
            let mut rgb = [0u8; 3];
            for k in 0..3 {
                let lit = text[k] * bright[i].clamp(0.0001, 1.0);
                let value = if alpha > 0.0 {
                    (lit * letters + dark[k] * behind * (1.0 - letters)) / alpha
                } else {
                    // See-through: the letters' colour, so edges blend clean.
                    text[k]
                };
                rgb[k] = value.round().clamp(0.0, 255.0) as u8;
            }
            let at = i * 4;
            frame.rgba[at..at + 3].copy_from_slice(&rgb);
            frame.rgba[at + 3] = (alpha * 255.0).round().clamp(0.0, 255.0) as u8;
        }
    }
    frame
}

/// Spreads `mask` by `reach` pixels each way (the larger value wins), for
/// letters a little heavier than the font's own.
fn heavier(mask: &mut [f32], cols: usize, rows: usize, reach: usize) {
    if reach == 0 {
        return;
    }
    let mut spread = vec![0.0f32; mask.len()];
    for row in 0..rows {
        let line = &mask[row * cols..(row + 1) * cols];
        for col in 0..cols {
            let (from, to) = (col.saturating_sub(reach), (col + reach + 1).min(cols));
            spread[row * cols + col] = line[from..to].iter().copied().fold(0.0, f32::max);
        }
    }
    for col in 0..cols {
        for row in 0..rows {
            let (from, to) = (row.saturating_sub(reach), (row + reach + 1).min(rows));
            mask[row * cols + col] = (from..to).map(|r| spread[r * cols + col]).fold(0.0, f32::max);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HD: (u32, u32) = (1920, 1080);

    fn alpha_at(frame: &media::Frame, x: u32, y: u32) -> u8 {
        frame.rgba[((y * frame.width + x) * 4 + 3) as usize]
    }

    #[test]
    fn a_lower_third_sits_low_on_the_left_and_a_title_in_the_middle() {
        let lower = bounds(&Title::new(TitleStyle::LowerThird), HD).expect("covers something");
        assert!(lower.left < 0.1 && lower.top > 0.6 && lower.top + lower.height < 0.95);
        let centred = bounds(&Title::new(TitleStyle::Centred), HD).expect("covers something");
        let middle = (
            centred.left + centred.width / 2.0,
            centred.top + centred.height / 2.0,
        );
        assert!((middle.0 - 0.5).abs() < 0.01 && (middle.1 - 0.5).abs() < 0.01);
        let card = bounds(&Title::new(TitleStyle::EndCard), HD).expect("covers something");
        assert_eq!(card, Part::WHOLE, "the card fills the frame");
    }

    #[test]
    fn longer_and_larger_words_cover_more() {
        let mut title = Title::new(TitleStyle::Centred);
        let short = bounds(&title, HD).expect("covers");
        title.text = "A much longer title".into();
        let long = bounds(&title, HD).expect("covers");
        assert!(long.width > short.width);
        title.size *= 2;
        assert!(bounds(&title, HD).expect("covers").height > long.height);
        title.text = "   ".into();
        assert_eq!(bounds(&title, HD), None, "nothing to show");
    }

    #[test]
    fn the_words_are_drawn_opaque_and_the_rest_see_through() {
        let title = Title::new(TitleStyle::Centred);
        let region = bounds(&title, HD).expect("covers");
        let size = ((region.width * 1920.0) as u32, (region.height * 1080.0) as u32);
        let layer = TitleLayer {
            title,
            frame: HD,
            region,
        };
        let frame = draw(&layer, size);
        assert_eq!((frame.width, frame.height), size);
        let solid = frame.rgba.chunks(4).filter(|p| p[3] == 255).count();
        let clear = frame.rgba.chunks(4).filter(|p| p[3] == 0).count();
        assert!(solid > 500, "letters: {solid}");
        assert!(clear > solid, "around them");
        assert_eq!(alpha_at(&frame, 0, 0), 0, "the corner is clear");
        // White letters, and white where it is clear so edges stay clean.
        assert!(frame.rgba.chunks(4).all(|p| p[..3] == [255, 255, 255]));
    }

    #[test]
    fn a_band_is_dark_and_part_see_through_behind_the_letters() {
        let title = Title::new(TitleStyle::LowerThird);
        let region = bounds(&title, HD).expect("covers");
        let layer = TitleLayer {
            title,
            frame: HD,
            region,
        };
        let frame = draw(&layer, (400, 100));
        // Its top left corner is band, not letters.
        let corner = &frame.rgba[(2 * 400 + 2) * 4..(2 * 400 + 2) * 4 + 4];
        assert_eq!(corner, [16, 16, 18, 153]);
    }
}
