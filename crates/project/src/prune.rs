//! Prune: the project's media that no timeline uses, and taking it out of
//! the project.
//!
//! A file counts as used when a clip on any timeline of the project plays
//! it, whether or not that timeline is open, placed in another one (a
//! nested timeline) or placed nowhere. Labels, lists and notes are not
//! use: they only organise files, and they go with a file that is pruned,
//! with its in/out marks (`Library::forget`; undo brings them back).
//!
//! Pruning only changes the project. Files on disk are never touched.

use crate::command::missing;
use crate::ids::MediaId;
use crate::model::Project;
use diag::AppError;
use std::collections::BTreeSet;

/// Every file a clip on some timeline of the project plays.
pub fn used_media(project: &Project) -> BTreeSet<MediaId> {
    project
        .sequences
        .iter()
        .flat_map(|s| s.tracks())
        .flat_map(|t| t.clips.iter())
        .filter(|c| c.is_file())
        .map(|c| c.media)
        .collect()
}

/// The project's files that no timeline uses, in id order (the order they
/// were added).
pub fn unused_media(project: &Project) -> Vec<MediaId> {
    let used = used_media(project);
    project
        .media
        .keys()
        .filter(|id| !used.contains(id))
        .copied()
        .collect()
}

/// Takes `media` out of the project, with its labels, its place in lists
/// and its note. Refused, with nothing changed, for an empty choice, a
/// file the project doesn't have or one that a timeline uses.
pub(crate) fn remove_media(p: &mut Project, media: &[MediaId]) -> diag::Result<()> {
    if media.is_empty() {
        return Err(AppError::new("No files were chosen")
            .why("Pruning takes out the files ticked in the list.")
            .fix("Tick the files to take out of the project."));
    }
    if media.iter().any(|m| !p.media.contains_key(m)) {
        return Err(missing("media file"));
    }
    let used = used_media(p);
    if let Some(in_use) = media.iter().find(|m| used.contains(m)) {
        let name = p.media.get(in_use).map_or_else(String::new, |m| {
            m.path.file_name().map_or_else(
                || m.path.display().to_string(),
                |n| n.to_string_lossy().into_owned(),
            )
        });
        return Err(AppError::new(format!("{name} is used on a timeline"))
            .why("Only files no timeline uses can be pruned; a clip still plays this one.")
            .fix("Remove its clips first, or leave it ticked off."));
    }
    for id in media {
        p.media.remove(id);
        p.library.forget(*id);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::Command;
    use crate::ids::{ClipId, SequenceId};
    use crate::library::MediaLabel;
    use crate::model::{Clip, Fingerprint, LabelColor, MediaKind, MediaRef, SequenceSettings};
    use crate::time::Time;

    fn file(name: &str) -> MediaRef {
        MediaRef {
            id: MediaId(0),
            path: format!("/recordings/{name}").into(),
            fingerprint: Fingerprint {
                size: 10,
                sample_hash: name.into(),
            },
            kind: MediaKind::Video,
            duration: Time::seconds(30),
            info: Default::default(),
        }
    }

    /// A project with three files: `a` on the first timeline, `b` only on
    /// a second timeline that is nested in the first, `c` on none.
    fn project() -> (Project, [MediaId; 3]) {
        let mut p = Project::new("Prune", SequenceSettings::GAME_CLIP);
        for name in ["a.mkv", "b.mkv", "c.mkv"] {
            p = Command::AddMedia { media: file(name) }.apply(&p).unwrap();
        }
        let ids: Vec<MediaId> = p.media.keys().copied().collect();
        let first = p.active_sequence;
        p = place(&p, first, ids[0]);
        p = Command::AddSequence {
            name: "Inner".into(),
            settings: SequenceSettings::GAME_CLIP,
        }
        .apply(&p)
        .unwrap();
        let inner = p.active_sequence;
        p = place(&p, inner, ids[1]);
        p = Command::PlaceSequence {
            sequence: first,
            nested: inner,
            at: None,
            track: None,
        }
        .apply(&p)
        .unwrap();
        (p, [ids[0], ids[1], ids[2]])
    }

    fn place(p: &Project, sequence: SequenceId, media: MediaId) -> Project {
        let track = p.sequence(sequence).unwrap().video_tracks[0].id;
        let start = p.sequence(sequence).unwrap().length();
        Command::PlaceClip {
            sequence,
            track,
            clip: Clip {
                lines: Default::default(),
                effects: Default::default(),
                mode: Default::default(),
                id: ClipId(0),
                media,
                stream: 0,
                start,
                source_in: Time::ZERO,
                duration: Time::seconds(5),
                link: None,
                speed: Default::default(),
                look: Default::default(),
                sound: Default::default(),
                keyframes: Default::default(),
                ramp: Default::default(),
                blend: Default::default(),
                kind: Default::default(),
            },
        }
        .apply(p)
        .unwrap()
    }

    #[test]
    fn a_file_used_only_in_a_nested_timeline_counts_as_used() {
        let (p, [a, b, c]) = project();
        assert_eq!(used_media(&p), BTreeSet::from([a, b]));
        assert_eq!(unused_media(&p), vec![c]);
    }

    #[test]
    fn a_timeline_placed_nowhere_still_uses_its_files() {
        let (p, [_, b, c]) = project();
        let first = p.sequences[0].id;
        // Take the nested clips off the first timeline: the inner timeline
        // is now placed nowhere, and its file still counts.
        let mut q = p.clone();
        let outer = &mut q.sequences[0];
        for track in outer.video_tracks.iter_mut().chain(outer.audio_tracks.iter_mut()) {
            track.clips.retain(|c| c.is_file());
        }
        assert!(q.sequence(first).is_some());
        assert!(used_media(&q).contains(&b));
        assert_eq!(unused_media(&q), vec![c]);
    }

    #[test]
    fn pruning_takes_the_file_and_what_organised_it_out_in_one_step() {
        let (p, [a, _, c]) = project();
        let p = Command::AddMediaLabel {
            label: MediaLabel {
                id: crate::ids::MediaLabelId(0),
                name: "Drone".into(),
                color: LabelColor::Blue,
            },
            media: vec![a, c],
        }
        .apply(&p)
        .unwrap();
        let p = Command::AddMediaList {
            name: "Maybe".into(),
            media: vec![c, a],
        }
        .apply(&p)
        .unwrap();
        let p = Command::SetMediaNote {
            media: c,
            note: "spare take".into(),
        }
        .apply(&p)
        .unwrap();

        let after = Command::RemoveMedia { media: vec![c] }.apply(&p).unwrap();
        assert!(!after.media.contains_key(&c));
        assert!(after.problems().is_empty(), "{:?}", after.problems());
        assert!(after.library.labels_of(c).is_empty());
        assert_eq!(
            after.library.labels_of(a).len(),
            1,
            "other files keep their labels"
        );
        assert_eq!(after.library.note(c), "");
        assert_eq!(
            after.library.lists[0].media.iter().copied().collect::<Vec<_>>(),
            vec![a]
        );
        assert_eq!(after.library.labels.len(), 1, "the label itself stays");
        assert!(unused_media(&after).is_empty());
    }

    #[test]
    fn used_unknown_and_empty_choices_are_refused() {
        let (p, [a, b, c]) = project();
        let err = Command::RemoveMedia { media: vec![c, a] }.apply(&p).unwrap_err();
        assert!(err.what().contains("a.mkv is used"), "{}", err.what());
        let err = Command::RemoveMedia { media: vec![b] }.apply(&p).unwrap_err();
        assert!(
            err.what().contains("b.mkv is used"),
            "nested use counts: {}",
            err.what()
        );
        assert!(
            Command::RemoveMedia {
                media: vec![MediaId(999)]
            }
            .apply(&p)
            .is_err()
        );
        assert!(Command::RemoveMedia { media: vec![] }.apply(&p).is_err());
    }

    #[test]
    fn undo_brings_every_pruned_file_back() {
        let (mut p, _) = project();
        p = Command::AddMedia { media: file("d.wav") }.apply(&p).unwrap();
        let mut history = crate::History::new(p.clone());
        history
            .apply(&Command::RemoveMedia {
                media: unused_media(&p),
            })
            .unwrap();
        assert_eq!(history.current().media.len(), 2);
        assert!(history.undo());
        assert_eq!(history.current(), &p);
    }
}
