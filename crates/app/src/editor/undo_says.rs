//! What an undo or redo changed, in words for the status line, and the
//! timeline it changed: when that is not the one on screen, the window
//! opens it (which timeline is open is view state, so this is no edit),
//! so a change never happens out of sight.

use super::Editor;
use imbl::Vector;
use project::{Project, Sequence, SequenceId, Track};
use std::collections::HashSet;

/// One step of the history as the person sees it.
#[derive(Debug, PartialEq)]
pub(crate) struct Step {
    /// The one timeline whose clips, tracks or labels the step changed,
    /// when it changed exactly one that is still there.
    pub timeline: Option<SequenceId>,
    /// What the change did, going forward ("removed 2 clips in Alt cut").
    pub says: String,
}

/// The change that takes `from` to `to`.
pub(crate) fn step(from: &Project, to: &Project) -> Step {
    let name = |id: SequenceId| {
        to.sequence(id)
            .or_else(|| from.sequence(id))
            .map_or_else(String::new, |s| s.name.clone())
    };
    let made: Vec<_> = to
        .sequences
        .iter()
        .filter(|s| from.sequence(s.id).is_none())
        .collect();
    let gone: Vec<_> = from
        .sequences
        .iter()
        .filter(|s| to.sequence(s.id).is_none())
        .collect();
    if let Some(s) = made.first() {
        return Step {
            timeline: Some(s.id),
            says: format!("made the timeline {}", s.name),
        };
    }
    if let Some(s) = gone.first() {
        return Step {
            timeline: None,
            says: format!("removed the timeline {}", s.name),
        };
    }
    let changed: Vec<(&Sequence, &Sequence)> = to
        .sequences
        .iter()
        .filter_map(|after| from.sequence(after.id).map(|before| (before, after)))
        .filter(|(before, after)| !same(before, after))
        .collect();
    match changed.as_slice() {
        [(before, after)] => Step {
            timeline: Some(after.id),
            says: format!("{} in {}", in_timeline(before, after), name(after.id)),
        },
        [] if from.media != to.media || from.library != to.library => Step {
            timeline: None,
            says: "changed the project's media".into(),
        },
        [] if from.name != to.name => Step {
            timeline: None,
            says: format!("renamed the project {}", to.name),
        },
        [] => Step {
            timeline: None,
            says: "a change".into(),
        },
        several => Step {
            timeline: None,
            says: format!("changed {} timelines", several.len()),
        },
    }
}

/// Whether a timeline is the same in both, open or not. A command shares
/// the tracks, clips and labels it leaves alone with the project before it,
/// so those are found the same by their pointers first; only what a pointer
/// says differs is compared in full (imbl's `==` has no such shortcut), so
/// an undo in a project of many large timelines stays quick.
fn same(a: &Sequence, b: &Sequence) -> bool {
    let rest = |s: &Sequence| Sequence {
        video_tracks: Vector::new(),
        audio_tracks: Vector::new(),
        labels: Vector::new(),
        ..s.clone()
    };
    rest(a) == rest(b)
        && same_tracks(&a.video_tracks, &b.video_tracks)
        && same_tracks(&a.audio_tracks, &b.audio_tracks)
        && (a.labels.ptr_eq(&b.labels) || a.labels == b.labels)
}

/// Whether two lists of tracks are the same, track by track.
fn same_tracks(a: &Vector<Track>, b: &Vector<Track>) -> bool {
    let same_track = |a: &Track, b: &Track| {
        let rest = |t: &Track| Track {
            clips: Vector::new(),
            ..t.clone()
        };
        (a.clips.ptr_eq(&b.clips) || a.clips == b.clips) && rest(a) == rest(b)
    };
    a.ptr_eq(b) || (a.len() == b.len() && a.iter().zip(b.iter()).all(|(a, b)| same_track(a, b)))
}

/// What changed inside one timeline, in a few words.
fn in_timeline(before: &Sequence, after: &Sequence) -> String {
    if before.name != after.name {
        return format!("renamed the timeline {}", after.name);
    }
    let ids = |s: &Sequence| {
        s.tracks()
            .flat_map(|t| t.clips.iter())
            .map(|c| c.id)
            .collect::<HashSet<_>>()
    };
    // Sets: an undo on a timeline of thousands of clips stays quick.
    let (old, new) = (ids(before), ids(after));
    let added = new.iter().filter(|c| !old.contains(c)).count();
    let removed = old.iter().filter(|c| !new.contains(c)).count();
    let clips = |n: usize| {
        if n == 1 {
            "1 clip".to_owned()
        } else {
            format!("{n} clips")
        }
    };
    let tracks = |s: &Sequence| s.tracks().count();
    match (added, removed) {
        (0, 0) if tracks(before) != tracks(after) => "changed the tracks".into(),
        (0, 0) if before.labels != after.labels => "changed the labels".into(),
        (0, 0) => "changed clips".into(),
        (a, 0) => format!("added {}", clips(a)),
        (0, r) => format!("removed {}", clips(r)),
        _ => "changed clips".into(),
    }
}

impl Editor {
    /// Undo (`forward` false) or redo, then: the timeline it changed is
    /// opened when another is on screen, and the status line says what was
    /// undone or redone, and where.
    pub(super) fn step_history(&mut self, forward: bool) -> diag::Result<()> {
        let Some(before) = self.document.as_ref().map(|d| d.project().clone()) else {
            return self.with_document(|_| Ok(()));
        };
        let mut moved = false;
        self.with_document(|d| {
            moved = if forward { d.redo()? } else { d.undo()? };
            Ok(())
        })?;
        let Some(document) = self.document.as_mut() else {
            return Ok(());
        };
        if !moved {
            self.status = if forward {
                "Nothing to redo.".into()
            } else {
                "Nothing to undo.".into()
            };
            return Ok(());
        }
        let after = document.project();
        // Undo reverses a change: say what the change itself did.
        let step = if forward {
            step(&before, after)
        } else {
            step(after, &before)
        };
        let open = after.active_sequence;
        let mut opened = String::new();
        if let Some(id) = step.timeline.filter(|id| *id != open)
            && document.open_sequence(id)
        {
            let name = document.project().sequence(id).map_or("", |s| s.name.as_str());
            opened = format!(" {name} is open now.");
        }
        let done = if forward { "Redone" } else { "Undone" };
        self.status = format!("{done}: {}.{opened}", step.says);
        Ok(())
    }
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use project::Command;

    #[test]
    fn a_step_names_what_changed_and_where() {
        let p = crate::inspector::tests::project_with_a_clip();
        let main = p.active_sequence;
        let other = Command::DuplicateSequence {
            sequence: main,
            name: "Alt cut".into(),
        }
        .apply(&p)
        .unwrap();
        let made = step(&p, &other);
        assert!(made.says.contains("Alt cut"), "{}", made.says);
        let alt = other.sequences.iter().find(|s| s.name == "Alt cut").unwrap().id;
        assert_eq!(made.timeline, Some(alt));
        // A clip removed there, with the main timeline open.
        let mut shown = other.clone();
        shown.active_sequence = main;
        let clip = shown.sequence(alt).unwrap().video_tracks[0].clips[0].id;
        let removed = Command::RemoveClips {
            sequence: alt,
            clips: vec![clip],
            ripple: false,
        }
        .apply(&shown)
        .unwrap();
        let s = step(&shown, &removed);
        assert_eq!(s.timeline, Some(alt), "the timeline to open");
        assert!(s.says.starts_with("removed "), "{}", s.says);
        assert!(s.says.ends_with("in Alt cut"), "{}", s.says);
    }

    /// The open timeline of `project_with_a_clip`, its one clip copied to
    /// `n` clips one after another, with the ids `first..first + n`.
    pub(crate) fn many_clips(first: u64, n: u64) -> Sequence {
        let p = crate::inspector::tests::project_with_a_clip();
        let mut seq = p.active().unwrap().clone();
        let clip = seq.video_tracks[0].clips[0].clone();
        let mut at = clip.start;
        seq.video_tracks[0].clips = (first..first + n)
            .map(|id| {
                let mut c = clip.clone();
                c.id = project::ClipId(id);
                c.start = at;
                at = c.end();
                c
            })
            .collect();
        seq
    }

    #[test]
    fn many_clips_added_or_removed_are_counted() {
        let before = many_clips(1000, 5000);
        let more = many_clips(1000, 5003);
        assert_eq!(in_timeline(&before, &more), "added 3 clips");
        assert_eq!(in_timeline(&more, &before), "removed 3 clips");
        let shifted = many_clips(1002, 5000);
        assert_eq!(in_timeline(&before, &shifted), "changed clips");
        let one_less = many_clips(1000, 4999);
        assert_eq!(in_timeline(&before, &one_less), "removed 1 clip");
        assert_eq!(in_timeline(&before, &before), "changed clips");
    }

    #[test]
    fn a_timeline_is_the_same_only_when_nothing_in_it_changed() {
        let a = many_clips(1000, 50);
        assert!(same(&a, &a.clone()));
        // Equal, but built afresh: no pointer is shared.
        let mut fresh = a.clone();
        fresh.video_tracks = a
            .video_tracks
            .iter()
            .map(|t| Track {
                clips: t.clips.iter().cloned().collect(),
                ..t.clone()
            })
            .collect();
        fresh.labels = a.labels.iter().cloned().collect();
        assert!(!fresh.video_tracks.ptr_eq(&a.video_tracks));
        assert!(same(&a, &fresh));
        let changed = |change: &dyn Fn(&mut Sequence)| {
            let mut b = a.clone();
            change(&mut b);
            !same(&a, &b) && !same(&b, &a)
        };
        assert!(changed(&|s| s.name = "Other".into()));
        assert!(changed(&|s| s.edit_needle = project::Time::seconds(3)));
        assert!(changed(
            &|s| s.video_tracks[0].clips[49].duration = project::Time::seconds(9)
        ));
        assert!(changed(&|s| {
            s.video_tracks[0].clips.pop_back();
        }));
        assert!(changed(&|s| s.video_tracks[0].muted = !s.video_tracks[0].muted));
        assert!(changed(&|s| s.audio_tracks[0].gain_db = -3.0));
        assert!(changed(&|s| {
            let track = s.video_tracks[0].clone();
            s.video_tracks.push_back(track);
        }));
        assert!(changed(&|s| {
            s.labels.push_back(project::TimelineLabel {
                id: project::LabelId(7),
                name: "Label 1".into(),
                at: project::Time::seconds(1),
                color: project::LabelColor::Blue,
            });
        }));
    }

    /// 20 timelines of 5 000 clips each and the project after one clip of
    /// the open one was removed.
    fn twenty_big_timelines() -> (Project, Project) {
        let mut p = crate::inspector::tests::project_with_a_clip();
        let open = p.active_sequence;
        let at = p.sequences.iter().position(|s| s.id == open).unwrap();
        p.sequences.set(at, many_clips(1000, 5000));
        for n in 0..19 {
            p = Command::DuplicateSequence {
                sequence: open,
                name: format!("Copy {n}"),
            }
            .apply(&p)
            .unwrap();
        }
        p.active_sequence = open;
        let after = Command::RemoveClips {
            sequence: open,
            clips: vec![project::ClipId(5999)],
            ripple: false,
        }
        .apply(&p)
        .unwrap();
        (p, after)
    }

    /// Prints how long finding the timeline an undo changed takes among 20
    /// timelines of 5 000 clips, against comparing every timeline in full.
    #[test]
    #[ignore = "a measurement: cargo test -p app -- --ignored --nocapture undo_finds"]
    fn undo_finds_the_changed_timeline_among_twenty_big_ones_quickly() {
        let (before, after) = twenty_big_timelines();
        let start = std::time::Instant::now();
        let says = step(&after, &before);
        let now = start.elapsed();
        let start = std::time::Instant::now();
        let full = after
            .sequences
            .iter()
            .filter(|s| before.sequence(s.id).is_some_and(|b| b != *s))
            .count();
        let comparing = start.elapsed();
        assert_eq!(says.timeline, Some(before.active_sequence));
        assert_eq!(full, 1);
        println!("undo finds the timeline, 20 x 5 000 clips: pointers {now:?}, full compare {comparing:?}");
    }

    /// Prints how long saying what an undo changed takes on a timeline of
    /// 5 000 clips, against the list-scanning way it was counted before.
    #[test]
    #[ignore = "a measurement: cargo test -p app -- --ignored --nocapture undo_says"]
    fn undo_says_on_five_thousand_clips_is_quick() {
        let before = many_clips(1000, 5000);
        let after = many_clips(1003, 5000);
        let start = std::time::Instant::now();
        let says = in_timeline(&before, &after);
        let now = start.elapsed();
        let ids = |s: &Sequence| {
            s.tracks()
                .flat_map(|t| t.clips.iter())
                .map(|c| c.id)
                .collect::<Vec<_>>()
        };
        let start = std::time::Instant::now();
        let (old, new) = (ids(&before), ids(&after));
        let added = new.iter().filter(|c| !old.contains(c)).count();
        let removed = old.iter().filter(|c| !new.contains(c)).count();
        let scanning = start.elapsed();
        assert_eq!((added, removed), (3, 3));
        assert_eq!(says, "changed clips");
        println!("undo says, 5 000 clips: sets {now:?}, list scans {scanning:?}");
    }
}
