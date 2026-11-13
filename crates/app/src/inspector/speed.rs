//! The inspector's Speed section: how fast the clip plays, one speed or a
//! ramp that changes over time, and whether slowed pictures blend frames.
//!
//! - **One speed:** the buttons set it for the whole clip (`SetSpeed`).
//! - **A ramp:** the diamond at the end of the Speed row works as it does
//!   for the picture's numbers: a click adds a speed keyframe at the
//!   playhead (the first one makes the speed change over time) or removes
//!   the one there, a right-click on a filled one says how the speed goes
//!   on to the next (linear, hold, smooth), and the arrows move the
//!   playhead to the previous or next one. With a ramp, the buttons set
//!   the speed at the playhead (a keyframe there). The two ramp buttons
//!   make one in a click, starting at the playhead: ease into slow motion
//!   and stay there, or ease in, stay a second and ease back out. Each is
//!   one `SetRamp`, one undo step.
//! - **Frame blending** (pictures only): off, or the two recorded frames
//!   around each moment mixed (`SetFrameBlend`).
//!
//! The clip always plays the same part of its file, so a change makes it
//! longer or shorter, and what follows moves along.

use project::ramp::{self, Ramp};
use project::{Clip, Command, Ease, FrameBlend, Rate, SequenceId, Speed, Time, TrackKind};
use ui_kit::Theme;
use ui_kit::keyframe::{self, KeyClicks, KeyState, Keys};

/// The speeds offered as buttons, as fractions.
const SPEEDS: [(u32, u32); 5] = [(1, 4), (1, 2), (1, 1), (2, 1), (4, 1)];

/// What the section is about: the clip and where it and the playhead are.
pub struct Subject<'a> {
    pub sequence: SequenceId,
    pub clip: &'a Clip,
    pub kind: TrackKind,
    pub rate: Rate,
    /// The rate its file was recorded at, when it has a picture.
    pub recorded: Option<Rate>,
    pub playhead: Time,
}

/// What the section asked for: commands (each an undo step) and a place
/// for the playhead.
#[derive(Debug, Default)]
pub struct Asked {
    pub commands: Vec<Command>,
    pub seek: Option<Time>,
}

/// Draws the Speed section.
pub fn section(ui: &mut egui::Ui, theme: &Theme, it: &Subject<'_>) -> Asked {
    let mut asked = Asked::default();
    let clip = it.clip;
    let keys = ramp_keys(it);
    let summary = clip.speed_summary();
    let changed = clip.speed != Speed::NORMAL || !clip.ramp.is_empty();
    let clicks = ui_kit::inspector::animated_section(
        ui,
        theme,
        "Speed",
        &summary,
        changed,
        !clip.ramp.is_empty(),
        |ui| body(ui, theme, it, &keys, &mut asked),
    );
    if clicks.reset {
        asked.commands.push(set_speed(it, Speed::NORMAL));
    }
    asked
}

fn body(ui: &mut egui::Ui, theme: &Theme, it: &Subject<'_>, keys: &RampKeys, asked: &mut Asked) {
    let clip = it.clip;
    let here = here(it);
    let now = clip.speed_at(clip.start + here.0);
    let small = |text: String| egui::RichText::new(text).color(theme.muted).size(12.0);

    // The speed at the playhead, with its keyframe control.
    let eases = Ease::ALL.map(Ease::label);
    let mut clicks = KeyClicks::default();
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new("Speed").color(theme.text_secondary));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            clicks = keyframe::control(ui, theme, "Speed", &keys.keys(&eases));
            ui.label(egui::RichText::new(now.percent()).font(ui_kit::fonts::mono(13.0)));
        });
    });
    answer_keys(it, keys, &clicks, now, asked);

    // One speed for the clip, or the speed at the playhead in a ramp.
    let mut speeds: Vec<Speed> = SPEEDS
        .iter()
        .filter_map(|(n, d)| Speed::new(*n, *d).ok())
        .collect();
    let clean = it
        .recorded
        .filter(|r| faster(*r, it.rate))
        .map(|r| ramp::clean_slow(r, it.rate));
    if let Some(clean) = clean
        && !speeds.contains(&clean)
    {
        speeds.insert(speeds.partition_point(|s| s.as_f64() < clean.as_f64()), clean);
    }
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        for speed in speeds {
            let tip = match (clip.ramp.is_empty(), Some(speed) == clean) {
                (true, true) => "The whole clip at this speed: every recorded frame shown once".to_owned(),
                (true, false) => "The whole clip at this speed".to_owned(),
                (false, _) => "This speed at the playhead (a speed keyframe there)".to_owned(),
            };
            let chosen = now == speed;
            if ui
                .selectable_label(chosen, speed.percent())
                .on_hover_text(tip)
                .clicked()
                && !chosen
            {
                asked.commands.push(match clip.ramp.is_empty() {
                    true => set_speed(it, speed),
                    false => {
                        let mut ramp = clip.ramp.clone();
                        ramp.set(here.0, here, speed);
                        set_ramp(it, ramp)
                    }
                });
            }
        }
    });

    // Ramps in a click, from the playhead.
    let slow = match (it.recorded, clean) {
        (_, Some(clean)) => clean,
        _ => Speed::new(1, 2).unwrap_or_default(),
    };
    ui.add_space(2.0);
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        let into = ui.button("Ease into slow motion").on_hover_text(format!(
            "From the playhead, ease down to {} over half a second and stay there",
            slow.percent()
        ));
        if into.clicked() {
            asked
                .commands
                .push(set_ramp(it, ramp_from_here(it, here.0, now, slow, false)));
        }
        let moment = ui.button("Slow moment").on_hover_text(format!(
            "From the playhead, ease down to {} over half a second, stay a second, and ease back to {}",
            slow.percent(),
            now.percent()
        ));
        if moment.clicked() {
            asked
                .commands
                .push(set_ramp(it, ramp_from_here(it, here.0, now, slow, true)));
        }
        if !clip.ramp.is_empty()
            && ui
                .button("Remove ramp")
                .on_hover_text(format!("Back to one speed for the whole clip: {}", now.percent()))
                .clicked()
        {
            asked.commands.push(set_speed(it, now));
        }
    });

    if it.kind == TrackKind::Video {
        ui.add_space(2.0);
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("Frame blending").color(theme.text_secondary));
            for (blend, name, tip) in [
                (
                    FrameBlend::Off,
                    "Off",
                    "Slowed past what the recording has, frames repeat",
                ),
                (
                    FrameBlend::Blend,
                    "Blend",
                    "Between two recorded frames, show both mixed by how near each is, so slow motion moves on every frame",
                ),
            ] {
                let chosen = clip.blend == blend;
                if ui.selectable_label(chosen, name).on_hover_text(tip).clicked() && !chosen {
                    asked.commands.push(Command::SetFrameBlend {
                        sequence: it.sequence,
                        clip: clip.id,
                        blend,
                    });
                }
            }
        });
    }
    let hint = match (it.recorded, clean) {
        (Some(recorded), Some(clean)) => format!(
            "Recorded at {}: at {} every recorded frame is shown once, no blending needed. The clip gets longer or shorter, with its sound, and what follows moves along.",
            recorded,
            clean.percent()
        ),
        _ => "The clip gets longer or shorter, with its sound (its pitch follows the speed), and what follows moves along."
            .to_owned(),
    };
    ui.label(small(hint));
}

/// Whether a recording at `recorded` has more frames than the timeline.
fn faster(recorded: Rate, timeline: Rate) -> bool {
    u64::from(recorded.num()) * u64::from(timeline.den())
        > u64::from(timeline.num()) * u64::from(recorded.den())
}

/// The timeline frame the playhead is in, brought onto the clip, as a
/// stretch from the clip's start (end excluded): what "at the playhead" means.
fn here(it: &Subject<'_>) -> (Time, Time) {
    let clip = it.clip;
    let frame = it.rate.frame_at(clip.moment(it.playhead, it.rate));
    let from = it.rate.frame_start(frame).max(clip.start) - clip.start;
    (from, it.rate.frame_start(frame + 1) - clip.start)
}

/// The clip's ramp with what was there from `at` on replaced by slow
/// motion starting there; what comes before `at` stays as it was, so the
/// picture up to the playhead doesn't change.
fn ramp_from_here(it: &Subject<'_>, at: Time, from: Speed, slow: Speed, back: bool) -> Ramp {
    let ease = it.rate.snap(Time::from_ratio(1, 2).unwrap_or(Time::ZERO));
    let hold = it.rate.snap(Time::seconds(1));
    let made = ramp::slow_motion(at, from, slow, ease, back.then_some(hold));
    let kept = it.clip.ramp.keys().iter().filter(|k| k.at < at).copied();
    Ramp::new(kept.chain(made.keys().iter().copied()))
}

fn set_speed(it: &Subject<'_>, speed: Speed) -> Command {
    Command::SetSpeed {
        sequence: it.sequence,
        clip: it.clip.id,
        speed,
    }
}

fn set_ramp(it: &Subject<'_>, ramp: Ramp) -> Command {
    Command::SetRamp {
        sequence: it.sequence,
        clip: it.clip.id,
        ramp,
    }
}

/// Where the clip's speed keyframes stand at the playhead.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RampKeys {
    state: KeyState,
    /// The nearest timeline frames with a keyframe before and after the
    /// playhead's, inside the clip.
    before: Option<i64>,
    after: Option<i64>,
    /// The ease of the keyframe at the playhead, as an index of `Ease::ALL`.
    ease: Option<usize>,
    /// The playhead is on the clip, so keyframes can be added and removed.
    can: bool,
}

impl RampKeys {
    fn keys<'a>(&self, eases: &'a [&'a str]) -> Keys<'a> {
        Keys {
            state: self.state,
            before: self.before.is_some(),
            after: self.after.is_some(),
            can: self.can,
            eases,
            ease: self.ease,
        }
    }
}

/// The diamond's state for `it`'s clip at the playhead.
pub fn ramp_keys(it: &Subject<'_>) -> RampKeys {
    let clip = it.clip;
    let can = clip.start <= it.playhead && it.playhead < clip.end();
    if clip.ramp.is_empty() {
        return RampKeys {
            can,
            ..RampKeys::default()
        };
    }
    let (first, end) = (it.rate.frame_at(clip.start), it.rate.frame_at(clip.end()));
    let frames: Vec<i64> = clip
        .ramp
        .keys()
        .iter()
        .map(|k| it.rate.frame_at(clip.start + k.at))
        .filter(|f| (first..end).contains(f))
        .collect();
    let near = here(it);
    let at = clip.ramp.keys().iter().find(|k| near.0 <= k.at && k.at < near.1);
    let frame = it.rate.frame_at(clip.start + near.0);
    RampKeys {
        state: if at.is_some() {
            KeyState::At
        } else {
            KeyState::Between
        },
        before: frames.iter().rev().find(|f| **f < frame).copied(),
        after: frames.iter().find(|f| **f > frame).copied(),
        ease: at.and_then(|k| Ease::ALL.iter().position(|e| *e == k.ease)),
        can,
    }
}

/// What the diamond and its arrows asked for.
fn answer_keys(it: &Subject<'_>, keys: &RampKeys, clicks: &KeyClicks, now: Speed, asked: &mut Asked) {
    let clip = it.clip;
    let near = here(it);
    if clicks.toggle {
        let mut ramp = clip.ramp.clone();
        match keys.state {
            KeyState::At => {
                ramp.remove(near);
                // The last keyframe gone: the clip keeps the speed it had.
                asked.commands.push(match ramp.is_empty() {
                    true => set_speed(it, now),
                    false => set_ramp(it, ramp),
                });
            }
            KeyState::Still | KeyState::Between => {
                ramp.set(near.0, near, now);
                asked.commands.push(set_ramp(it, ramp));
            }
        }
    }
    if let Some(ease) = clicks.ease.and_then(|i| Ease::ALL.get(i)) {
        let mut ramp = clip.ramp.clone();
        if ramp.set_ease(near, *ease) {
            asked.commands.push(set_ramp(it, ramp));
        }
    }
    let jump = match (clicks.previous, clicks.next) {
        (true, _) => keys.before,
        (_, true) => keys.after,
        _ => None,
    };
    if let Some(frame) = jump {
        asked.seek = Some(it.rate.frame_start(frame));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use project::{ClipId, SpeedKey};

    fn clip() -> Clip {
        Clip {
            lines: Default::default(),
            id: ClipId(7),
            media: project::MediaId(1),
            stream: 0,
            start: Time::seconds(10),
            source_in: Time::ZERO,
            duration: Time::seconds(10),
            link: None,
            speed: Speed::NORMAL,
            look: Default::default(),
            sound: Default::default(),
            keyframes: Default::default(),
            ramp: Default::default(),
            blend: Default::default(),
            kind: Default::default(),
        }
    }

    fn subject(clip: &Clip, playhead: Time) -> Subject<'_> {
        Subject {
            sequence: SequenceId(1),
            clip,
            kind: TrackKind::Video,
            rate: Rate::FPS_60,
            recorded: Some(Rate::FPS_120),
            playhead,
        }
    }

    #[test]
    fn the_diamond_says_where_the_speed_keyframes_are() {
        let plain = clip();
        let keys = ramp_keys(&subject(&plain, Time::seconds(12)));
        assert_eq!((keys.state, keys.can), (KeyState::Still, true));
        let ramped = Clip {
            ramp: Ramp::new([
                SpeedKey {
                    at: Time::seconds(1),
                    speed: Speed::NORMAL,
                    ease: Ease::Smooth,
                },
                SpeedKey {
                    at: Time::seconds(3),
                    speed: Speed::new(1, 2).unwrap(),
                    ease: Ease::Linear,
                },
            ]),
            ..clip()
        };
        let at = ramp_keys(&subject(&ramped, Time::seconds(11)));
        assert_eq!(at.state, KeyState::At);
        assert_eq!(at.ease, Some(2), "smooth");
        assert_eq!(at.after, Some(13 * 60));
        let between = ramp_keys(&subject(&ramped, Time::seconds(12)));
        assert_eq!(between.state, KeyState::Between);
        assert_eq!((between.before, between.after), (Some(11 * 60), Some(13 * 60)));
    }

    #[test]
    fn the_diamond_adds_and_removes_and_the_last_one_leaves_its_speed() {
        let plain = clip();
        let it = subject(&plain, Time::seconds(12));
        let mut asked = Asked::default();
        let toggle = KeyClicks {
            toggle: true,
            ..KeyClicks::default()
        };
        answer_keys(&it, &ramp_keys(&it), &toggle, Speed::NORMAL, &mut asked);
        let Some(Command::SetRamp { ramp, .. }) = asked.commands.pop() else {
            panic!("a ramp with one keyframe");
        };
        assert_eq!(ramp.keys().len(), 1);
        assert_eq!(ramp.keys()[0].at, Time::seconds(2));
        let keyed = Clip { ramp, ..clip() };
        let it = subject(&keyed, Time::seconds(12));
        let half = Speed::new(1, 2).unwrap();
        answer_keys(&it, &ramp_keys(&it), &toggle, half, &mut asked);
        assert!(
            matches!(asked.commands.pop(), Some(Command::SetSpeed { speed, .. }) if speed == half),
            "removing the last keyframe keeps the speed it gave"
        );
    }

    #[test]
    fn a_ramp_from_the_playhead_keeps_what_comes_before_it() {
        let ramped = Clip {
            ramp: ramp::slow_motion(
                Time::seconds(1),
                Speed::NORMAL,
                Speed::new(1, 4).unwrap(),
                Time::from_ratio(1, 2).unwrap(),
                Some(Time::seconds(1)),
            ),
            ..clip()
        };
        let it = subject(&ramped, Time::seconds(14));
        let ramp = ramp_from_here(
            &it,
            Time::seconds(4),
            Speed::NORMAL,
            Speed::new(1, 2).unwrap(),
            false,
        );
        let times: Vec<Time> = ramp.keys().iter().map(|k| k.at).collect();
        let half = Time::from_ratio(1, 2).unwrap();
        assert_eq!(
            times,
            [
                Time::seconds(1),
                Time::seconds(1) + half,
                Time::seconds(2) + half,
                Time::seconds(3),
                Time::seconds(4),
                Time::seconds(4) + half
            ]
        );
        assert!(faster(Rate::FPS_120, Rate::FPS_60) && !faster(Rate::FPS_60, Rate::FPS_60));
    }
}
