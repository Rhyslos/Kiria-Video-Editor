//! The files chosen on the Library page: a click chooses one, Ctrl-click
//! adds or takes one away, Shift-click chooses the run from the last one
//! clicked, Select all (the shortcut table's, Ctrl+A) every file shown.
//! What is done to the chosen files (labels, lists, proxies, adding them,
//! taking them out of the project) is one undo step for them all.

use project::{MediaId, Project};

/// The chosen files, in the order they were chosen, and the one a
/// Shift-click runs from.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Chosen {
    files: Vec<MediaId>,
    anchor: Option<MediaId>,
}

impl Chosen {
    pub fn files(&self) -> &[MediaId] {
        &self.files
    }

    pub fn contains(&self, media: MediaId) -> bool {
        self.files.contains(&media)
    }

    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    /// Several files are chosen.
    pub fn several(&self) -> bool {
        self.files.len() > 1
    }

    /// Only this file.
    pub fn only(&mut self, media: MediaId) {
        self.files = vec![media];
        self.anchor = Some(media);
    }

    /// A click on `media`, `order` being the files as shown: alone, with
    /// Ctrl (`toggle`) it is added or taken away, with Shift (`run`) every
    /// file from the last one clicked to it is chosen. Returns the file the
    /// detail panel shows now (none when the last one was taken away).
    pub fn click(&mut self, media: MediaId, toggle: bool, run: bool, order: &[MediaId]) -> Option<MediaId> {
        if run && let Some(anchor) = self.anchor {
            let at = |m: MediaId| order.iter().position(|o| *o == m);
            if let (Some(from), Some(to)) = (at(anchor), at(media)) {
                let (a, b) = (from.min(to), from.max(to));
                let run = &order[a..=b];
                if !toggle {
                    self.files.clear();
                }
                for m in run {
                    if !self.files.contains(m) {
                        self.files.push(*m);
                    }
                }
                return Some(media);
            }
        }
        if toggle {
            self.anchor = Some(media);
            if let Some(at) = self.files.iter().position(|m| *m == media) {
                self.files.remove(at);
                return self.files.last().copied();
            }
            self.files.push(media);
            return Some(media);
        }
        self.only(media);
        Some(media)
    }

    /// Every file shown.
    pub fn all(&mut self, order: &[MediaId]) {
        self.files = order.to_vec();
        self.anchor = order.first().copied();
    }

    /// Files no longer in the project leave the choice.
    pub fn keep_existing(&mut self, project: &Project) {
        self.files.retain(|m| project.media.contains_key(m));
        if self.anchor.is_some_and(|m| !project.media.contains_key(&m)) {
            self.anchor = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(n: &[u64]) -> Vec<MediaId> {
        n.iter().map(|i| MediaId(*i)).collect()
    }

    #[test]
    fn clicks_with_ctrl_and_shift_choose_as_in_any_file_list() {
        let order = ids(&[1, 2, 3, 4, 5]);
        let mut chosen = Chosen::default();
        assert_eq!(chosen.click(MediaId(1), false, false, &order), Some(MediaId(1)));
        chosen.click(MediaId(2), true, false, &order);
        chosen.click(MediaId(3), true, false, &order);
        assert_eq!(chosen.files(), ids(&[1, 2, 3]), "Ctrl adds");
        assert_eq!(
            chosen.click(MediaId(2), true, false, &order),
            Some(MediaId(3)),
            "Ctrl again takes it away; the panel shows the last still chosen"
        );
        assert_eq!(chosen.files(), ids(&[1, 3]));
        chosen.click(MediaId(5), false, true, &order);
        assert_eq!(
            chosen.files(),
            ids(&[2, 3, 4, 5]),
            "Shift: the run from the last clicked"
        );
        chosen.click(MediaId(1), true, true, &order);
        assert_eq!(chosen.files(), ids(&[2, 3, 4, 5, 1]), "Ctrl+Shift adds the run");
        chosen.click(MediaId(4), false, false, &order);
        assert_eq!(chosen.files(), ids(&[4]), "a plain click: that one alone");
        assert!(!chosen.several());
    }

    #[test]
    fn select_all_takes_every_file_shown() {
        let mut chosen = Chosen::default();
        chosen.all(&ids(&[7, 8, 9]));
        assert_eq!(chosen.files().len(), 3);
        assert!(chosen.several());
        let mut p = Project::new_automatic("T");
        p.media.insert(
            MediaId(8),
            project::MediaRef {
                id: MediaId(8),
                path: "/r/8.mp4".into(),
                fingerprint: project::Fingerprint {
                    size: 1,
                    sample_hash: String::new(),
                },
                kind: project::MediaKind::Video,
                duration: project::Time::seconds(1),
                info: project::MediaInfo::default(),
            },
        );
        chosen.keep_existing(&p);
        assert_eq!(chosen.files(), ids(&[8]), "files taken out of the project leave");
    }
}
