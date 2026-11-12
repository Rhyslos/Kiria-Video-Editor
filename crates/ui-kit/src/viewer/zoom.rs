//! Zooming into the viewer's picture: Fit (the usual), or a scale where
//! 100 % shows one pixel of the timeline on one pixel of the screen.
//!
//! Pure maths on rectangles, so it is the same on every page and tested
//! without a window. The viewer ([`super::zoomed_viewer`]) reads the hand
//! (Ctrl+wheel over the picture zooms at the pointer, a drag with the
//! middle button pans) and draws the frame where [`Zoom::frame_rect`] says.

use egui::{Pos2, Rect, Vec2, vec2};

/// The scales the zoom commands step through, smallest first.
pub const STEPS: [f32; 11] = [0.1, 0.25, 1.0 / 3.0, 0.5, 2.0 / 3.0, 1.0, 1.5, 2.0, 3.0, 4.0, 8.0];

/// The scales the zoom menu offers by name, besides Fit.
pub const MENU: [f32; 3] = [0.5, 1.0, 2.0];

/// The smallest and largest scale.
pub const LEAST: f32 = 0.1;
pub const MOST: f32 = 8.0;

/// How the viewer's picture is zoomed. View state: kept by the page while
/// it runs, never in the project.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Zoom {
    /// Screen pixels per timeline pixel; `None` fits the frame in the viewer.
    scale: Option<f32>,
    /// The point of the frame at the middle of the viewer, as shares of
    /// the frame's width and height (0.5, 0.5 is its middle).
    centre: Vec2,
}

impl Default for Zoom {
    fn default() -> Zoom {
        Zoom::FIT
    }
}

impl Zoom {
    /// The whole frame, as large as fits (the usual).
    pub const FIT: Zoom = Zoom {
        scale: None,
        centre: vec2(0.5, 0.5),
    };

    /// `scale` (1.0 is 100 %), about the frame's middle.
    pub fn at(scale: f32) -> Zoom {
        Zoom {
            scale: Some(scale.clamp(LEAST, MOST)),
            centre: vec2(0.5, 0.5),
        }
    }

    pub fn is_fit(&self) -> bool {
        self.scale.is_none()
    }

    /// Zoomed to `scale` (not Fit, even where Fit comes to the same).
    pub fn is_at(&self, scale: f32) -> bool {
        self.scale.is_some_and(|s| (s - scale).abs() < 1e-3)
    }

    /// The scale that fits a frame of `timeline` pixels inside `area`
    /// (points) on a screen with `ppp` pixels per point.
    pub fn fit_scale(area: Rect, timeline: (u32, u32), ppp: f32) -> f32 {
        let (w, h) = (timeline.0.max(1) as f32, timeline.1.max(1) as f32);
        (area.width() / w).min(area.height() / h).max(0.0) * ppp
    }

    /// The scale in use: the fitting one for Fit.
    pub fn scale(&self, area: Rect, timeline: (u32, u32), ppp: f32) -> f32 {
        self.scale.unwrap_or_else(|| Zoom::fit_scale(area, timeline, ppp))
    }

    /// The frame's rectangle on screen for a viewer showing `area`: inside
    /// it when fitted or small, beyond it when zoomed in (the viewer clips).
    /// A frame larger than the area along a side always covers it there,
    /// so panning never shows past the frame's edge; a smaller one is
    /// centred.
    pub fn frame_rect(&self, area: Rect, timeline: (u32, u32), ppp: f32) -> Rect {
        let Some(scale) = self.scale else {
            return super::picture_rect(area, timeline);
        };
        let size = frame_size(timeline, scale, ppp);
        let centre = kept_in(self.centre, size, area.size());
        Rect::from_min_size(area.center() - centre * size, size)
    }

    /// The scale written for people: "Fit" or "200 %" (the scale in use
    /// beside Fit's name when `with_fit_scale`, "Fit · 47 %").
    pub fn readout(&self, area: Rect, timeline: (u32, u32), ppp: f32, with_fit_scale: bool) -> String {
        let percent = |scale: f32| format!("{} %", (scale * 100.0).round() as i64);
        match self.scale {
            Some(scale) => percent(scale),
            None if with_fit_scale => format!("Fit · {}", percent(Zoom::fit_scale(area, timeline, ppp))),
            None => "Fit".to_owned(),
        }
    }

    /// Zooms to `scale`, keeping the frame's point under `at` (a place on
    /// screen) where it is.
    pub fn zoom_to_at(&mut self, scale: f32, at: Pos2, area: Rect, timeline: (u32, u32), ppp: f32) {
        let before = self.frame_rect(area, timeline, ppp);
        let scale = scale.clamp(LEAST, MOST);
        let size = frame_size(timeline, scale, ppp);
        // The frame's point under the pointer, as shares of the frame.
        let held = (at - before.min) / before.size().max(vec2(1e-3, 1e-3));
        let min = at - held * size;
        let centre = (area.center() - min) / size.max(vec2(1e-3, 1e-3));
        *self = Zoom {
            scale: Some(scale),
            centre: kept_in(centre, size, area.size()),
        };
    }

    /// Zooms by `factor` (above 1 in, below 1 out) at `at`. Zooming out
    /// across the fitting scale stops at Fit, so the wheel finds it; going
    /// on from Fit zooms out further.
    pub fn zoom_by_at(&mut self, factor: f32, at: Pos2, area: Rect, timeline: (u32, u32), ppp: f32) {
        let fit = Zoom::fit_scale(area, timeline, ppp);
        let now = self.scale(area, timeline, ppp);
        let to = now * factor;
        let across_fit = (factor < 1.0 && now > fit && to <= fit) || (factor > 1.0 && now < fit && to >= fit);
        if across_fit {
            *self = Zoom::FIT;
        } else {
            self.zoom_to_at(to, at, area, timeline, ppp);
        }
    }

    /// One step in (`inward`) or out along [`STEPS`], about the middle of
    /// the viewer. From Fit it steps from the fitting scale.
    pub fn step(&mut self, inward: bool, area: Rect, timeline: (u32, u32), ppp: f32) {
        let now = self.scale(area, timeline, ppp);
        // A little room, so a scale a hair off a step counts as on it.
        let next = if inward {
            STEPS.iter().copied().find(|s| *s > now * 1.01)
        } else {
            STEPS.iter().rev().copied().find(|s| *s < now / 1.01)
        };
        if let Some(scale) = next {
            self.zoom_to_at(scale, area.center(), area, timeline, ppp);
        }
    }

    /// Moves the picture by `by` points (a drag), within what the frame
    /// allows.
    pub fn pan(&mut self, by: Vec2, area: Rect, timeline: (u32, u32), ppp: f32) {
        let Some(scale) = self.scale else {
            return;
        };
        let size = frame_size(timeline, scale, ppp);
        let centre = self.centre - by / size.max(vec2(1e-3, 1e-3));
        self.centre = kept_in(centre, size, area.size());
    }

    /// Whether the picture is bigger than `area` along a side: there is
    /// somewhere to pan to.
    pub fn can_pan(&self, area: Rect, timeline: (u32, u32), ppp: f32) -> bool {
        let frame = self.frame_rect(area, timeline, ppp);
        frame.width() > area.width() + 0.5 || frame.height() > area.height() + 0.5
    }
}

/// A frame of `timeline` pixels at `scale`, in points.
fn frame_size(timeline: (u32, u32), scale: f32, ppp: f32) -> Vec2 {
    vec2(timeline.0.max(1) as f32, timeline.1.max(1) as f32) * scale / ppp.max(0.1)
}

/// `centre` (shares of a frame `size` big) kept where the frame covers a
/// viewer `room` big along each side it is larger, and in the middle along
/// each side it is not.
fn kept_in(centre: Vec2, size: Vec2, room: Vec2) -> Vec2 {
    let keep = |c: f32, size: f32, room: f32| {
        if size <= room {
            0.5
        } else {
            let half = room / 2.0 / size;
            c.clamp(half, 1.0 - half)
        }
    };
    vec2(keep(centre.x, size.x, room.x), keep(centre.y, size.y, room.y))
}

/// What part of `picture` (a rectangle on screen) shows inside `clip`:
/// the rectangle on screen and the same part as shares of the picture
/// (left, top, right, bottom), for drawing only what is seen. `None` when
/// nothing of it is.
pub fn seen_part(picture: Rect, clip: Rect) -> Option<(Rect, Rect)> {
    let seen = picture.intersect(clip);
    if seen.width() <= 0.0 || seen.height() <= 0.0 || picture.width() <= 0.0 || picture.height() <= 0.0 {
        return None;
    }
    let share = |p: Pos2| {
        Pos2::new(
            (p.x - picture.min.x) / picture.width(),
            (p.y - picture.min.y) / picture.height(),
        )
    };
    Some((seen, Rect::from_min_max(share(seen.min), share(seen.max))))
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::pos2;

    /// A viewer 800 × 450 points on a screen with 1 pixel per point, and
    /// a 1920 × 1080 timeline (fitted at 41.67 %).
    const TL: (u32, u32) = (1920, 1080);

    fn area() -> Rect {
        Rect::from_min_size(pos2(100.0, 50.0), vec2(800.0, 450.0))
    }

    fn near(a: f32, b: f32) -> bool {
        (a - b).abs() < 0.01
    }

    #[test]
    fn fit_is_the_whole_frame_inside_the_viewer() {
        let fit = Zoom::FIT.frame_rect(area(), TL, 1.0);
        assert_eq!(fit, crate::viewer::picture_rect(area(), TL));
        assert!(near(Zoom::fit_scale(area(), TL, 1.0), 800.0 / 1920.0));
        assert_eq!(Zoom::default(), Zoom::FIT);
        assert!(Zoom::default().is_fit());
        assert_eq!(Zoom::FIT.readout(area(), TL, 1.0, false), "Fit");
        assert_eq!(Zoom::FIT.readout(area(), TL, 1.0, true), "Fit · 42 %");
        assert!(!Zoom::FIT.can_pan(area(), TL, 1.0));
    }

    #[test]
    fn a_hundred_percent_is_one_timeline_pixel_on_one_screen_pixel() {
        let rect = Zoom::at(1.0).frame_rect(area(), TL, 1.0);
        assert_eq!(rect.size(), vec2(1920.0, 1080.0));
        assert_eq!(rect.center(), area().center(), "about the middle");
        // On a screen with 2 pixels per point it is half as many points.
        let sharp = Zoom::at(1.0).frame_rect(area(), TL, 2.0);
        assert_eq!(sharp.size(), vec2(960.0, 540.0));
        assert_eq!(Zoom::at(2.0).readout(area(), TL, 1.0, true), "200 %");
        assert!(Zoom::at(1.0).can_pan(area(), TL, 1.0));
        // A small scale is centred, nothing to pan.
        let small = Zoom::at(0.25).frame_rect(area(), TL, 1.0);
        assert_eq!(small.center(), area().center());
        assert!(!Zoom::at(0.25).can_pan(area(), TL, 1.0));
        assert_eq!(Zoom::at(100.0), Zoom::at(MOST), "never past the largest");
    }

    #[test]
    fn zooming_at_the_pointer_keeps_what_is_under_it_there() {
        let pointer = pos2(250.0, 120.0);
        let mut zoom = Zoom::FIT;
        let before = zoom.frame_rect(area(), TL, 1.0);
        let held = (pointer - before.min) / before.size();
        zoom.zoom_by_at(2.0, pointer, area(), TL, 1.0);
        let after = zoom.frame_rect(area(), TL, 1.0);
        let still = (pointer - after.min) / after.size();
        assert!(
            near(held.x, still.x) && near(held.y, still.y),
            "{held:?} {still:?}"
        );
        assert!(near(zoom.scale(area(), TL, 1.0), 2.0 * 800.0 / 1920.0));
        // Near an edge the frame stays covering the viewer: what was under
        // the pointer moves rather than showing past the frame.
        let mut corner = Zoom::FIT;
        let top_left = picture_corner();
        corner.zoom_by_at(3.0, top_left, area(), TL, 1.0);
        let rect = corner.frame_rect(area(), TL, 1.0);
        assert!(rect.min.x <= area().min.x + 0.01 && rect.min.y <= area().min.y + 0.01);
        assert!(rect.max.x >= area().max.x && rect.max.y >= area().max.y);
    }

    fn picture_corner() -> Pos2 {
        Zoom::FIT.frame_rect(area(), TL, 1.0).min
    }

    #[test]
    fn zooming_out_with_the_wheel_stops_at_fit_once() {
        let centre = area().center();
        let mut zoom = Zoom::at(0.5);
        zoom.zoom_by_at(0.8, centre, area(), TL, 1.0);
        assert!(zoom.is_fit(), "0.4 is past the fit at 0.42");
        zoom.zoom_by_at(0.8, centre, area(), TL, 1.0);
        assert!(!zoom.is_fit(), "then on, smaller");
        assert!(zoom.scale(area(), TL, 1.0) < Zoom::fit_scale(area(), TL, 1.0));
        // And back in across it.
        zoom.zoom_by_at(1.5, centre, area(), TL, 1.0);
        assert!(zoom.is_fit());
        zoom.zoom_by_at(0.001, centre, area(), TL, 1.0);
        assert!(near(zoom.scale(area(), TL, 1.0), LEAST), "never below the least");
    }

    #[test]
    fn the_commands_step_through_the_usual_scales() {
        let mut zoom = Zoom::FIT; // 41.67 %
        zoom.step(true, area(), TL, 1.0);
        assert!(near(zoom.scale(area(), TL, 1.0), 0.5));
        zoom.step(true, area(), TL, 1.0);
        assert!(near(zoom.scale(area(), TL, 1.0), 2.0 / 3.0));
        zoom.step(false, area(), TL, 1.0);
        zoom.step(false, area(), TL, 1.0);
        assert!(near(zoom.scale(area(), TL, 1.0), 1.0 / 3.0));
        let mut top = Zoom::at(MOST);
        top.step(true, area(), TL, 1.0);
        assert_eq!(top, Zoom::at(MOST), "nothing past the last step");
        assert_eq!(STEPS.first(), Some(&LEAST));
        assert_eq!(STEPS.last(), Some(&MOST));
        assert!(STEPS.windows(2).all(|w| w[0] < w[1]));
        assert!(MENU.iter().all(|s| STEPS.contains(s)));
    }

    #[test]
    fn panning_moves_the_picture_with_the_hand_and_stops_at_its_edges() {
        let mut zoom = Zoom::at(1.0);
        let before = zoom.frame_rect(area(), TL, 1.0);
        zoom.pan(vec2(100.0, -40.0), area(), TL, 1.0);
        let after = zoom.frame_rect(area(), TL, 1.0);
        assert!(near(after.min.x - before.min.x, 100.0) && near(after.min.y - before.min.y, -40.0));
        // As far as it goes: the frame's left edge stops at the viewer's.
        zoom.pan(vec2(5000.0, 0.0), area(), TL, 1.0);
        let left = zoom.frame_rect(area(), TL, 1.0);
        assert!(near(left.min.x, area().min.x));
        // Fit doesn't pan.
        let mut fit = Zoom::FIT;
        fit.pan(vec2(30.0, 30.0), area(), TL, 1.0);
        assert_eq!(fit, Zoom::FIT);
        // A smaller viewer later: the frame is kept covering it.
        let smaller = Rect::from_min_size(area().min, vec2(400.0, 300.0));
        let kept = zoom.frame_rect(smaller, TL, 1.0);
        assert!(kept.min.x <= smaller.min.x && kept.max.x >= smaller.max.x);
    }

    #[test]
    fn only_the_seen_part_of_a_picture_is_drawn() {
        let picture = Rect::from_min_size(pos2(0.0, 0.0), vec2(400.0, 200.0));
        let clip = Rect::from_min_max(pos2(100.0, 50.0), pos2(1000.0, 150.0));
        let (seen, part) = seen_part(picture, clip).unwrap();
        assert_eq!(seen, Rect::from_min_max(pos2(100.0, 50.0), pos2(400.0, 150.0)));
        assert_eq!(part, Rect::from_min_max(pos2(0.25, 0.25), pos2(1.0, 0.75)));
        let whole = seen_part(picture, picture.expand(10.0)).unwrap().1;
        assert_eq!(whole, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)));
        assert!(seen_part(picture, Rect::from_min_size(pos2(500.0, 0.0), vec2(10.0, 10.0))).is_none());
    }
}
