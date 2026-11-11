//! Scrub traces: where the pointer puts the playhead, and when. Synthesised,
//! so every approach and every machine replays exactly the same movement.

/// The playhead was put at `pos` (seconds into the clip) at time `at` (seconds).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Request {
    pub at: f64,
    pub pos: f64,
}

pub struct Trace {
    pub name: &'static str,
    pub requests: Vec<Request>,
}

/// Pointer positions are read this often.
const POLLS_PER_SECOND: f64 = 60.0;

/// Samples `offset(t)` (seconds into an 18-second stretch of the clip) for
/// `seconds`. Positions land on the middle of a frame, and a request is
/// only sent when the frame changes, as the viewer does.
fn sample(span: f64, fps: f64, seconds: f64, per_second: f64, offset: impl Fn(f64) -> f64) -> Vec<Request> {
    // Use the middle 18 s of long clips; squeeze the movement into short ones.
    let base = ((span - 18.0) / 2.0).max(0.0);
    let squeeze = (span / 18.0).min(1.0);
    let last_frame = ((span - 0.25).max(0.0) * fps).floor();
    let mut requests: Vec<Request> = Vec::new();
    let mut previous = -1.0;
    for i in 0..(seconds * per_second).round() as usize {
        let at = i as f64 / per_second;
        // A hair is added so that times meant to be on a frame boundary don't round below it.
        let frame = ((base + offset(at) * squeeze) * fps + 1e-6)
            .floor()
            .clamp(0.0, last_frame);
        if frame != previous {
            requests.push(Request {
                at,
                pos: (frame + 0.5) / fps,
            });
            previous = frame;
        }
    }
    requests
}

/// The traces every approach replays on a clip `span` seconds long.
/// `quick` shortens them for a smoke run.
pub fn all(span: f64, fps: f64, quick: bool) -> Vec<Trace> {
    let length = |seconds: f64| if quick { seconds / 4.0 } else { seconds };
    let triangle = |x: f64| 1.0 - (x.rem_euclid(2.0) - 1.0).abs();
    let trace = |name, seconds: f64, per_second: f64, offset: &dyn Fn(f64) -> f64| Trace {
        name,
        requests: sample(span, fps, length(seconds), per_second, offset),
    };
    vec![
        // Dragging the playhead at twice normal speed, then letting go.
        trace("slow drag", 4.0, POLLS_PER_SECOND, &|t| 2.0 + 2.0 * t),
        trace("slow drag back", 4.0, POLLS_PER_SECOND, &|t| 12.0 - 2.0 * t),
        // Sweeping 16 s of video each second, there and back and there again.
        trace("fast drag", 3.0, POLLS_PER_SECOND, &|t| 1.0 + 16.0 * triangle(t)),
        // Looking for a cut point: 0.4 s either side of one spot.
        trace("back and forth", 4.0, POLLS_PER_SECOND, &|t| {
            9.3 + 0.4 * (t * 1.25 * std::f64::consts::TAU).sin()
        }),
        // The left arrow key, 8 presses a second, across a key frame (16 s
        // into the test clips, which have one every 2 s or every 8 s).
        trace("step back", 5.0, 8.0, &|t| 15.25 - (t * 8.0).round() / fps),
        // Playing backwards at normal speed, across the same key frame.
        trace("play back", 3.0, POLLS_PER_SECOND, &|t| 16.0 - t),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn traces_stay_inside_the_clip_and_run_forward_in_time() {
        for span in [3.0, 20.0, 600.0] {
            for trace in all(span, 60.0, false) {
                assert!(!trace.requests.is_empty(), "{}", trace.name);
                for pair in trace.requests.windows(2) {
                    assert!(pair[0].at < pair[1].at, "{}", trace.name);
                    assert!(
                        pair[0].pos != pair[1].pos,
                        "{}: a request per new frame",
                        trace.name
                    );
                }
                for r in &trace.requests {
                    assert!(r.pos > 0.0 && r.pos < span, "{} at {span}", trace.name);
                }
            }
        }
    }

    #[test]
    fn stepping_back_moves_one_frame_a_press() {
        let traces = all(20.0, 60.0, false);
        let steps = traces.iter().find(|t| t.name == "step back").unwrap();
        assert_eq!(steps.requests.len(), 40);
        for pair in steps.requests.windows(2) {
            assert!((pair[0].pos - pair[1].pos - 1.0 / 60.0).abs() < 1e-9);
            assert!((pair[1].at - pair[0].at - 0.125).abs() < 1e-9);
        }
    }

    #[test]
    fn playing_back_asks_for_every_frame() {
        let traces = all(20.0, 60.0, false);
        let play = traces.iter().find(|t| t.name == "play back").unwrap();
        assert_eq!(play.requests.len(), 180);
    }
}
