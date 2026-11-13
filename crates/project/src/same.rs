//! Telling whether a part of a project changed, cheaply enough for every
//! frame.
//!
//! Snapshots share what didn't change, so comparing pointers is usually
//! enough. A vector short enough to be kept inline (a few small items: the
//! media labels, the custom lists) is copied rather than shared when a
//! project is cloned, so its pointers never match: those are compared by
//! contents, which costs no more than the pointers would.

use crate::model::Project;
use imbl::Vector;

/// Whether `a` and `b` hold the same items: shared, or short and equal.
pub fn same_vector<A: Clone + PartialEq>(a: &Vector<A>, b: &Vector<A>) -> bool {
    a.ptr_eq(b) || (a.is_inline() && b.is_inline() && a == b)
}

impl Project {
    /// Whether the media, the library (labels, tags, notes, lists) and the
    /// timelines are those of `other`. A pointer comparison, not a deep one.
    pub fn same_media_library_and_timelines(&self, other: &Project) -> bool {
        self.media.ptr_eq(&other.media)
            && same_vector(&self.sequences, &other.sequences)
            && same_vector(&self.library.labels, &other.library.labels)
            && self.library.tagged.ptr_eq(&other.library.tagged)
            && self.library.notes.ptr_eq(&other.library.notes)
            && same_vector(&self.library.lists, &other.library.lists)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::LabelColor;
    use crate::{Command, MediaLabel, MediaLabelId};

    #[test]
    fn a_clone_is_the_same_and_an_edit_is_not() {
        let p = Project::new_automatic("Trip");
        assert!(p.clone().same_media_library_and_timelines(&p), "nothing yet");
        let labelled = Command::AddMediaLabel {
            label: MediaLabel {
                id: MediaLabelId(0),
                name: "Drone".into(),
                color: LabelColor::Blue,
            },
            media: Vec::new(),
        }
        .apply(&p)
        .unwrap();
        assert!(labelled.clone().same_media_library_and_timelines(&labelled));
        assert!(!labelled.same_media_library_and_timelines(&p));
    }
}
