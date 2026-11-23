//! Small pictures of what each effect, transition and title does, drawn
//! with shapes rather than read from files: a little scene (sky, ground,
//! sun) with the effect's mark on it. Cheap to draw every frame and the
//! same in every theme.

use egui::{Color32, Painter, Pos2, Rect, Stroke, pos2, vec2};
use project::{EffectKind, TitleStyle, TransitionKind};

use super::Item;

const SKY: Color32 = Color32::from_rgb(46, 64, 92);
const GROUND: Color32 = Color32::from_rgb(54, 96, 72);
const SUN: Color32 = Color32::from_rgb(240, 196, 92);
/// The second shot of a transition: a warmer scene.
const SKY_B: Color32 = Color32::from_rgb(120, 64, 72);
const GROUND_B: Color32 = Color32::from_rgb(150, 110, 70);

/// Draws `item`'s picture in `r`.
pub fn draw(p: &Painter, r: Rect, item: Item) {
    let p = p.with_clip_rect(r.intersect(p.clip_rect()));
    match item {
        Item::Effect(kind) => {
            scene(&p, r, SKY, GROUND);
            effect(&p, r, kind);
        }
        Item::Transition(kind) => transition(&p, r, kind),
        Item::Title(style) => title(&p, r, style),
    }
    p.rect_stroke(
        r,
        4.0,
        Stroke::new(1.0, Color32::from_white_alpha(24)),
        egui::StrokeKind::Inside,
    );
}

/// A point at a share of `r`'s width and height.
fn at(r: Rect, x: f32, y: f32) -> Pos2 {
    pos2(r.left() + r.width() * x, r.top() + r.height() * y)
}

fn scene(p: &Painter, r: Rect, sky: Color32, ground: Color32) {
    p.rect_filled(r, 4.0, sky);
    let land = Rect::from_min_max(at(r, 0.0, 0.66), r.max);
    p.rect_filled(land, 0.0, ground);
    p.circle_filled(at(r, 0.7, 0.36), r.height() * 0.14, SUN);
}

fn effect(p: &Painter, r: Rect, kind: EffectKind) {
    let sun = at(r, 0.7, 0.36);
    match kind {
        EffectKind::Vignette(_) => {
            for i in 0..6 {
                let inset = i as f32 * r.height() * 0.05;
                let alpha = 70 - i * 11;
                p.rect_stroke(
                    r.shrink(inset),
                    4.0,
                    Stroke::new(r.height() * 0.06, Color32::from_black_alpha(alpha as u8)),
                    egui::StrokeKind::Inside,
                );
            }
        }
        EffectKind::Grain(_) => {
            for i in 0..60u32 {
                let x = (i.wrapping_mul(2_654_435_761) % 997) as f32 / 997.0;
                let y = (i.wrapping_mul(40_503).wrapping_add(17) % 991) as f32 / 991.0;
                p.circle_filled(at(r, x, y), 0.8, Color32::from_white_alpha(90));
            }
        }
        EffectKind::Sharpen(_) => {
            p.circle_stroke(sun, r.height() * 0.14, Stroke::new(1.2, Color32::WHITE));
            p.line_segment(
                [at(r, 0.0, 0.66), at(r, 1.0, 0.66)],
                Stroke::new(1.2, Color32::WHITE),
            );
        }
        EffectKind::Glow(_) => {
            for i in 1..5 {
                p.circle_filled(
                    sun,
                    r.height() * (0.14 + 0.06 * i as f32),
                    Color32::from_rgba_unmultiplied(255, 220, 140, 34),
                );
            }
        }
        EffectKind::Blur(_) => soft_box(p, Rect::from_min_max(at(r, 0.15, 0.25), at(r, 0.5, 0.75))),
        EffectKind::Pixelate(_) => {
            let shades = [90u8, 130, 70, 150, 110, 80];
            let cell = vec2(r.width() * 0.12, r.height() * 0.2);
            for (i, shade) in shades.iter().enumerate() {
                let corner = at(r, 0.14, 0.3) + vec2(cell.x * (i % 3) as f32, cell.y * (i / 3) as f32);
                p.rect_filled(Rect::from_min_size(corner, cell), 0.0, Color32::from_gray(*shade));
            }
        }
        EffectKind::Zoom(_) => {
            let inner = Rect::from_center_size(sun, r.size() * 0.4);
            let stroke = Stroke::new(1.2, Color32::WHITE);
            p.rect_stroke(inner, 2.0, stroke, egui::StrokeKind::Inside);
            for (a, b) in [
                (r.left_top(), inner.left_top()),
                (r.right_top(), inner.right_top()),
                (r.left_bottom(), inner.left_bottom()),
                (r.right_bottom(), inner.right_bottom()),
            ] {
                p.line_segment([a, b], Stroke::new(1.0, Color32::from_white_alpha(140)));
            }
        }
        EffectKind::Shake(_) => {
            for (dx, dy) in [(-3.0, 1.0), (3.0, -1.5)] {
                p.circle_filled(
                    sun + vec2(dx, dy),
                    r.height() * 0.14,
                    Color32::from_rgba_unmultiplied(240, 196, 92, 90),
                );
            }
            p.line_segment(
                [at(r, 0.1, 0.5), at(r, 0.3, 0.45)],
                Stroke::new(1.0, Color32::WHITE),
            );
            p.line_segment(
                [at(r, 0.1, 0.56), at(r, 0.3, 0.6)],
                Stroke::new(1.0, Color32::WHITE),
            );
        }
        EffectKind::Pan(_) => arrow(p, at(r, 0.15, 0.5), at(r, 0.85, 0.5)),
        EffectKind::Mirror(_) => {
            // The sun again on the other side, and the line it turns over.
            p.circle_filled(at(r, 0.3, 0.36), r.height() * 0.14, SUN.gamma_multiply(0.7));
            p.line_segment(
                [at(r, 0.5, 0.08), at(r, 0.5, 0.92)],
                Stroke::new(1.2, Color32::from_white_alpha(200)),
            );
        }
        EffectKind::Corners(_) => {
            // The shape it leaves: round at the corners.
            let inner = r.shrink(r.height() * 0.12);
            p.rect_stroke(
                inner,
                r.height() * 0.3,
                Stroke::new(1.5, Color32::WHITE),
                egui::StrokeKind::Inside,
            );
        }
        EffectKind::Outline(_) => {
            let inner = r.shrink(r.height() * 0.14);
            p.rect_stroke(
                inner,
                4.0,
                Stroke::new(2.0, Color32::WHITE),
                egui::StrokeKind::Outside,
            );
        }
        EffectKind::Shadow(_) => {
            let card = Rect::from_min_max(at(r, 0.22, 0.18), at(r, 0.66, 0.66));
            p.rect_filled(
                card.translate(vec2(4.0, 4.0)),
                3.0,
                Color32::from_black_alpha(140),
            );
            p.rect_filled(card, 3.0, Color32::from_rgb(196, 204, 220));
        }
        EffectKind::Key(_) => {
            // Half still the screen, half keyed: the scene shows through
            // round a figure.
            let half = Rect::from_min_max(r.min, at(r, 0.5, 1.0));
            p.rect_filled(half, 0.0, Color32::from_rgb(40, 180, 70));
            let figure = Color32::from_rgb(214, 170, 140);
            p.circle_filled(at(r, 0.5, 0.42), r.height() * 0.13, figure);
            p.rect_filled(
                Rect::from_min_max(at(r, 0.36, 0.6), at(r, 0.64, 1.0)),
                6.0,
                Color32::from_rgb(70, 80, 110),
            );
        }
        EffectKind::Mono(_) | EffectKind::Sepia(_) | EffectKind::Duotone(_) | EffectKind::Invert(_) => {
            // The scene again in the look's colours.
            let (sky, ground, light) = match kind {
                EffectKind::Mono(_) => (
                    Color32::from_gray(60),
                    Color32::from_gray(80),
                    Color32::from_gray(200),
                ),
                EffectKind::Sepia(_) => (
                    Color32::from_rgb(96, 74, 52),
                    Color32::from_rgb(112, 88, 60),
                    Color32::from_rgb(230, 200, 150),
                ),
                EffectKind::Duotone(_) => (
                    Color32::from_rgb(70, 30, 120),
                    Color32::from_rgb(110, 50, 110),
                    Color32::from_rgb(255, 170, 60),
                ),
                _ => (
                    Color32::from_rgb(209, 191, 163),
                    Color32::from_rgb(201, 159, 183),
                    Color32::from_rgb(15, 59, 163),
                ),
            };
            scene(p, r, sky, ground);
            p.circle_filled(sun, r.height() * 0.14, light);
        }
        EffectKind::Split(_) => {
            // The sun in red and blue, drawn apart.
            let size = r.height() * 0.14;
            p.circle_filled(
                sun - vec2(3.0, 0.0),
                size,
                Color32::from_rgba_unmultiplied(255, 40, 40, 140),
            );
            p.circle_filled(
                sun + vec2(3.0, 0.0),
                size,
                Color32::from_rgba_unmultiplied(40, 120, 255, 140),
            );
        }
        EffectKind::Letterbox(_) => {
            let bar = r.height() * 0.2;
            p.rect_filled(
                Rect::from_min_size(r.min, vec2(r.width(), bar)),
                0.0,
                Color32::BLACK,
            );
            p.rect_filled(
                Rect::from_min_max(r.max - vec2(r.width(), bar), r.max),
                0.0,
                Color32::BLACK,
            );
        }
        EffectKind::Stabilise(_) => {
            let middle = r.center();
            let stroke = Stroke::new(1.3, Color32::WHITE);
            p.circle_stroke(middle, r.height() * 0.22, stroke);
            p.line_segment(
                [
                    middle - vec2(r.height() * 0.35, 0.0),
                    middle + vec2(r.height() * 0.35, 0.0),
                ],
                stroke,
            );
            p.line_segment(
                [
                    middle - vec2(0.0, r.height() * 0.35),
                    middle + vec2(0.0, r.height() * 0.35),
                ],
                stroke,
            );
        }
    }
}

/// A box drawn soft, as blurred.
fn soft_box(p: &Painter, b: Rect) {
    for i in 0..4 {
        p.rect_filled(
            b.expand(i as f32 * 1.5),
            3.0,
            Color32::from_gray(160).gamma_multiply(0.25),
        );
    }
    p.rect_filled(b, 3.0, Color32::from_gray(150));
}

fn arrow(p: &Painter, from: Pos2, to: Pos2) {
    let stroke = Stroke::new(1.6, Color32::WHITE);
    p.line_segment([from, to], stroke);
    let back = (from - to).normalized() * 6.0;
    let side = vec2(-back.y, back.x) * 0.6;
    p.line_segment([to, to + back + side], stroke);
    p.line_segment([to, to + back - side], stroke);
}

fn transition(p: &Painter, r: Rect, kind: TransitionKind) {
    let half = |left: bool| match left {
        true => Rect::from_min_max(r.min, at(r, 0.5, 1.0)),
        false => Rect::from_min_max(at(r, 0.5, 0.0), r.max),
    };
    match kind {
        TransitionKind::Dissolve => {
            scene(p, r, SKY, GROUND);
            // The second shot over it, stronger to the right.
            let steps = 12;
            for i in 0..steps {
                let x0 = i as f32 / steps as f32;
                let strip = Rect::from_min_max(at(r, x0, 0.0), at(r, x0 + 1.0 / steps as f32, 1.0));
                let alpha = (x0 * 255.0) as u8;
                p.rect_filled(
                    strip,
                    0.0,
                    Color32::from_rgba_unmultiplied(150, 110, 70, alpha / 2),
                );
            }
        }
        TransitionKind::DipToBlack => {
            scene(p, half(true), SKY, GROUND);
            scene(p, half(false), SKY_B, GROUND_B);
            let middle = Rect::from_min_max(at(r, 0.35, 0.0), at(r, 0.65, 1.0));
            p.rect_filled(middle, 0.0, Color32::BLACK);
        }
        TransitionKind::Wipe => {
            scene(p, half(true), SKY_B, GROUND_B);
            scene(p, half(false), SKY, GROUND);
            p.line_segment(
                [at(r, 0.5, 0.0), at(r, 0.5, 1.0)],
                Stroke::new(1.5, Color32::WHITE),
            );
        }
        TransitionKind::Push => {
            scene(p, half(true), SKY, GROUND);
            scene(p, half(false), SKY_B, GROUND_B);
            arrow(p, at(r, 0.85, 0.5), at(r, 0.25, 0.5));
        }
        TransitionKind::Zoom => {
            scene(p, r, SKY, GROUND);
            let inner = Rect::from_center_size(r.center(), r.size() * 0.5);
            scene(p, inner, SKY_B, GROUND_B);
        }
    }
}

fn title(p: &Painter, r: Rect, style: TitleStyle) {
    let bar = |b: Rect| p.rect_filled(b, 1.0, Color32::WHITE);
    match style {
        TitleStyle::LowerThird => {
            scene(p, r, SKY, GROUND);
            let band = Rect::from_min_max(at(r, 0.06, 0.62), at(r, 0.6, 0.9));
            p.rect_filled(band, 2.0, Color32::from_black_alpha(150));
            bar(Rect::from_min_max(at(r, 0.1, 0.67), at(r, 0.45, 0.75)));
            bar(Rect::from_min_max(at(r, 0.1, 0.79), at(r, 0.35, 0.84)));
        }
        TitleStyle::Centred => {
            scene(p, r, SKY, GROUND);
            bar(Rect::from_center_size(
                r.center(),
                vec2(r.width() * 0.6, r.height() * 0.16),
            ));
        }
        TitleStyle::EndCard => {
            p.rect_filled(r, 4.0, Color32::from_rgb(16, 16, 18));
            bar(Rect::from_center_size(
                r.center(),
                vec2(r.width() * 0.5, r.height() * 0.12),
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_item_has_a_picture() {
        let ctx = egui::Context::default();
        let items: Vec<Item> = EffectKind::ALL
            .into_iter()
            .map(Item::Effect)
            .chain(TransitionKind::ALL.into_iter().map(Item::Transition))
            .chain(TitleStyle::ALL.into_iter().map(Item::Title))
            .collect();
        let output = ctx.run_ui(egui::RawInput::default(), |ui| {
            for (i, item) in items.iter().enumerate() {
                let r = Rect::from_min_size(pos2(0.0, i as f32 * 40.0), vec2(60.0, 36.0));
                draw(ui.painter(), r, *item);
            }
        });
        let shapes = output.shapes.len();
        // Nothing draws this frame.
        output.drop_without_applying_deltas();
        assert!(shapes >= items.len() * 3, "a scene and a frame each");
    }
}
