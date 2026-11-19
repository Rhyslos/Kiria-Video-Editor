//! Linking and unlinking clips: a picture and its sound streams come in
//! linked (selected, moved, trimmed and split together); unlinked, each
//! plays and is worked on its own. See [`crate::Command::Unlink`],
//! [`crate::Command::Link`] and [`crate::Command::UnlinkAll`].

use crate::command::{missing, sequence_mut, tracks_mut};
use crate::ids::{ClipId, LinkId, SequenceId};
use crate::model::Project;
use diag::AppError;
use std::collections::HashSet;

/// See [`crate::Command::Unlink`]: `clips` and every clip linked to any of
/// them end up with no link. Refused when none of them is linked.
pub(crate) fn unlink(p: &mut Project, sequence: SequenceId, clips: &[ClipId]) -> diag::Result<()> {
    let seq = sequence_mut(p, sequence)?;
    let wanted: HashSet<ClipId> = clips.iter().copied().collect();
    let mut found = 0;
    let mut links = HashSet::new();
    for clip in seq.tracks().flat_map(|t| t.clips.iter()) {
        if wanted.contains(&clip.id) {
            found += 1;
            links.extend(clip.link);
        }
    }
    if found < wanted.len() {
        return Err(missing("clip"));
    }
    if links.is_empty() {
        return Err(AppError::new("Nothing to unlink")
            .why("These clips are not linked to any other clip.")
            .fix("Choose a clip that moves with its sound or picture."));
    }
    for track in tracks_mut(seq) {
        for clip in track.clips.iter_mut() {
            if clip.link.is_some_and(|l| links.contains(&l)) {
                clip.link = None;
            }
        }
    }
    Ok(())
}

/// See [`crate::Command::Link`].
pub(crate) fn link(p: &mut Project, sequence: SequenceId, clips: &[ClipId]) -> diag::Result<()> {
    let wanted: HashSet<ClipId> = clips.iter().copied().collect();
    if wanted.len() < 2 {
        return Err(AppError::new("Two clips or more are linked")
            .why("A link joins clips so they are worked on together.")
            .fix("Select the picture and its sound (Shift-click), then link them."));
    }
    let link: LinkId = p.allocate_id();
    let seq = sequence_mut(p, sequence)?;
    let mut found = 0;
    for track in tracks_mut(seq) {
        for clip in track.clips.iter_mut().filter(|c| wanted.contains(&c.id)) {
            clip.link = Some(link);
            found += 1;
        }
    }
    if found < wanted.len() {
        return Err(missing("clip"));
    }
    Ok(())
}

/// See [`crate::Command::UnlinkAll`].
pub(crate) fn unlink_all(p: &mut Project) {
    for seq in p.sequences.iter_mut() {
        for track in tracks_mut(seq) {
            for clip in track.clips.iter_mut().filter(|c| c.link.is_some()) {
                clip.link = None;
            }
        }
    }
}

/// Whether any clip of `p` is linked to another.
pub fn has_links(p: &Project) -> bool {
    p.sequences
        .iter()
        .any(|s| s.tracks().any(|t| t.clips.iter().any(|c| c.link.is_some())))
}

#[cfg(test)]
mod tests {
    use crate::command::Command;
    use crate::command::tests::imported;
    use crate::ids::{ClipId, LinkId};
    use crate::model::Project;

    /// Every clip of the open timeline with its link, pictures first.
    fn links(p: &Project) -> Vec<(ClipId, Option<LinkId>)> {
        p.active()
            .unwrap()
            .tracks()
            .flat_map(|t| t.clips.iter().map(|c| (c.id, c.link)))
            .collect()
    }

    /// The first recording's picture and its two sound streams (Game, Mic).
    fn first_three(p: &Project) -> Vec<ClipId> {
        let first = links(p)[0].1;
        links(p)
            .into_iter()
            .filter(|c| c.1 == first)
            .map(|c| c.0)
            .collect()
    }

    #[test]
    fn unlinking_one_clip_sets_the_picture_and_every_sound_stream_apart() {
        let (p, sequence) = imported();
        let ids = first_three(&p);
        assert_eq!(ids.len(), 3, "{:?}", links(&p));
        let apart = Command::Unlink {
            sequence,
            clips: vec![ids[1]],
        }
        .apply(&p)
        .unwrap();
        for (id, link) in links(&apart) {
            if ids.contains(&id) {
                assert_eq!(link, None, "the two sound streams apart from each other too");
            } else {
                assert!(link.is_some(), "the second recording stays linked");
            }
        }
        // Nothing linked left: refused, as is a clip that isn't there.
        let one = |c: ClipId| Command::Unlink {
            sequence,
            clips: vec![c],
        };
        assert!(one(ids[0]).apply(&apart).is_err());
        assert!(one(ClipId::from(999_999)).apply(&p).is_err());
        // Two linked again: those two move together, the third alone.
        let two = Command::Link {
            sequence,
            clips: vec![ids[0], ids[2]],
        }
        .apply(&apart)
        .unwrap();
        let l: Vec<_> = links(&two)
            .into_iter()
            .filter(|c| ids.contains(&c.0))
            .map(|c| c.1)
            .collect();
        assert!(l[0].is_some() && l[0] == l[2] && l[1].is_none(), "{l:?}");
        let alone = Command::Link {
            sequence,
            clips: vec![ids[0]],
        };
        assert!(alone.apply(&apart).is_err());
        let none = Command::UnlinkAll.apply(&p).unwrap();
        assert!(!crate::linking::has_links(&none));
    }

    #[test]
    fn steps_are_one_change_all_or_nothing() {
        let (p, sequence) = imported();
        let ids = first_three(&p);
        let both = Command::Steps(vec![
            Command::Unlink {
                sequence,
                clips: vec![ids[0]],
            },
            Command::Link {
                sequence,
                clips: vec![ids[1], ids[2]],
            },
        ])
        .apply(&p)
        .unwrap();
        let l: Vec<_> = links(&both)
            .into_iter()
            .filter(|c| ids.contains(&c.0))
            .map(|c| c.1)
            .collect();
        assert!(l[0].is_none() && l[1].is_some() && l[1] == l[2], "{l:?}");
        // The second refused: nothing of the first is kept.
        let refused = Command::Steps(vec![
            Command::UnlinkAll,
            Command::Unlink {
                sequence,
                clips: vec![ids[0]],
            },
        ]);
        assert!(refused.apply(&p).is_err());
    }
}
