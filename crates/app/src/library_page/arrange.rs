//! What the Library page shows, worked out without drawing anything: which
//! files the open tab holds and how many have each label or match each
//! smart filter (`Facts`), and the files the index found, filtered and
//! grouped into the lines the grid or the rows are drawn in (`Arranged`),
//! each line's top worked out once, so only the lines in view are drawn.

use super::{GroupBy, Smart, Tab};
use library::Type;
use project::{LabelColor, MediaId, MediaLabelId, MediaRef, Project};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

/// What the sidebar counts, over the files of the open tab, and which
/// files any timeline uses (with how many clips), over the whole project.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Facts {
    /// Clips playing each file, on every timeline of the project.
    pub clips: HashMap<MediaId, u32>,
    /// Timelines using each file.
    pub timelines: HashMap<MediaId, u32>,
    /// Files of the tab with each label.
    pub labels: HashMap<MediaLabelId, usize>,
    /// Files of the tab matching each smart filter.
    pub smart: HashMap<Smart, usize>,
    /// Files of each type in the whole project, for the tabs.
    pub types: HashMap<Type, usize>,
    /// Files in the tab.
    pub in_tab: usize,
}

impl Facts {
    /// Counts `project`'s files. `needs_proxy` and `missing` say what the
    /// project alone can't.
    pub fn of(
        project: &Project,
        tab: Tab,
        needs_proxy: &dyn Fn(MediaId) -> bool,
        missing: &HashSet<MediaId>,
    ) -> Facts {
        let mut facts = Facts::default();
        for seq in &project.sequences {
            let mut here = HashSet::new();
            for clip in seq.tracks().flat_map(|t| t.clips.iter()).filter(|c| c.is_file()) {
                *facts.clips.entry(clip.media).or_default() += 1;
                here.insert(clip.media);
            }
            for media in here {
                *facts.timelines.entry(media).or_default() += 1;
            }
        }
        for media in project.media.values() {
            *facts.types.entry(media.kind.into()).or_default() += 1;
            if !tab.holds(project, media) {
                continue;
            }
            facts.in_tab += 1;
            if let Some(on) = project.library.tagged.get(&media.id) {
                for label in on {
                    *facts.labels.entry(*label).or_default() += 1;
                }
            }
            for smart in Smart::ALL {
                if smart.matches(project, media, &facts.clips, needs_proxy, missing) {
                    *facts.smart.entry(smart).or_default() += 1;
                }
            }
        }
        facts
    }

    /// Whether a timeline uses the file.
    pub fn used(&self, media: MediaId) -> bool {
        self.clips.contains_key(&media)
    }
}

impl Tab {
    /// Whether this tab holds `media`: a type tab its files of that type, a
    /// list its members (any type).
    pub fn holds(self, project: &Project, media: &MediaRef) -> bool {
        match self {
            Tab::Type(t) => Type::from(media.kind) == t,
            Tab::List(list) => project
                .library
                .list(list)
                .is_some_and(|l| l.media.contains(&media.id)),
        }
    }
}

impl Smart {
    /// Whether `media` matches this smart filter.
    pub fn matches(
        self,
        project: &Project,
        media: &MediaRef,
        clips: &HashMap<MediaId, u32>,
        needs_proxy: &dyn Fn(MediaId) -> bool,
        missing: &HashSet<MediaId>,
    ) -> bool {
        match self {
            Smart::NotUsed => !clips.contains_key(&media.id),
            Smart::NeedsProxy => needs_proxy(media.id),
            Smart::EndMissing => media.info.stated_length.is_some(),
            Smart::Missing => missing.contains(&media.id),
            Smart::WithNote => !project.library.note(media.id).is_empty(),
        }
    }
}

/// One line of the page's list: a group's heading, or a row of files
/// (`items[from..to]`: a row of cards in the grid, one file in rows).
#[derive(Debug, Clone, PartialEq)]
pub enum Line {
    Heading {
        title: String,
        colour: Option<LabelColor>,
        count: usize,
    },
    Files {
        from: usize,
        to: usize,
    },
}

/// The files found, in the lines they are drawn in.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Arranged {
    /// Every file shown, group after group.
    pub items: Vec<MediaId>,
    pub lines: Vec<Line>,
    /// Where each line begins, from the top of the list.
    pub tops: Vec<f32>,
    pub height: f32,
}

/// Sizes the lines are laid out with.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sizes {
    /// Files side by side in a row (1 for rows).
    pub columns: usize,
    pub row: f32,
    pub heading: f32,
}

/// The heading of files without a label.
pub const NO_LABEL: &str = "No label";

/// Groups `found` (in the index's order, which each group keeps) as
/// `group` says and lays them out in lines.
pub fn arrange(found: &[MediaId], project: &Project, group: GroupBy, sizes: Sizes) -> Arranged {
    let groups = grouped(found, project, group);
    let mut arranged = Arranged::default();
    let columns = sizes.columns.max(1);
    let mut top = 0.0;
    for (heading, files) in groups {
        if let Some((title, colour)) = heading {
            arranged.lines.push(Line::Heading {
                title,
                colour,
                count: files.len(),
            });
            arranged.tops.push(top);
            top += sizes.heading;
        }
        let start = arranged.items.len();
        arranged.items.extend(files);
        let end = arranged.items.len();
        let mut from = start;
        while from < end {
            let to = (from + columns).min(end);
            arranged.lines.push(Line::Files { from, to });
            arranged.tops.push(top);
            top += sizes.row;
            from = to;
        }
    }
    arranged.height = top;
    arranged
}

impl Arranged {
    /// The lines that reach into the stretch from `top` to `bottom`.
    pub fn in_view(&self, top: f32, bottom: f32, sizes: Sizes) -> std::ops::Range<usize> {
        let height = |i: usize| match self.lines[i] {
            Line::Heading { .. } => sizes.heading,
            Line::Files { .. } => sizes.row,
        };
        let first = self.tops.partition_point(|t| *t <= top).saturating_sub(1);
        let mut last = first;
        while last < self.lines.len() && self.tops[last] < bottom {
            last += 1;
        }
        // The line before `top` may end above it.
        let first = if first < self.lines.len() && self.tops[first] + height(first) <= top {
            first + 1
        } else {
            first
        };
        first.min(last)..last
    }
}

type Heading = Option<(String, Option<LabelColor>)>;

/// The groups, in order, each with its heading (none when not grouping).
fn grouped(found: &[MediaId], project: &Project, group: GroupBy) -> Vec<(Heading, Vec<MediaId>)> {
    let files = found.iter().filter(|id| project.media.contains_key(*id));
    match group {
        GroupBy::Nothing => vec![(None, files.copied().collect())],
        GroupBy::Label => {
            let library = &project.library;
            // Under its first label, in the order labels were made.
            let order: HashMap<MediaLabelId, usize> = library
                .labels
                .iter()
                .enumerate()
                .map(|(i, l)| (l.id, i))
                .collect();
            let mut groups: Vec<Vec<MediaId>> = vec![Vec::new(); library.labels.len() + 1];
            for id in files {
                let first = library
                    .tagged
                    .get(id)
                    .and_then(|on| on.iter().filter_map(|l| order.get(l)).min().copied());
                groups[first.unwrap_or(library.labels.len())].push(*id);
            }
            let mut out = Vec::new();
            for (i, files) in groups.into_iter().enumerate() {
                if files.is_empty() {
                    continue;
                }
                let heading = match library.labels.get(i) {
                    Some(label) => (label.name.clone(), Some(label.color)),
                    None => (NO_LABEL.to_owned(), None),
                };
                out.push((Some(heading), files));
            }
            out
        }
        GroupBy::Folder => {
            let mut groups: Vec<(PathBuf, Vec<MediaId>)> = Vec::new();
            for id in files {
                let Some(media) = project.media.get(id) else {
                    continue;
                };
                let folder = media.path.parent().map(PathBuf::from).unwrap_or_default();
                match groups.iter_mut().find(|(f, _)| *f == folder) {
                    Some((_, files)) => files.push(*id),
                    None => groups.push((folder, vec![*id])),
                }
            }
            groups.sort_by_key(|(f, _)| f.to_string_lossy().to_lowercase());
            groups
                .into_iter()
                .map(|(folder, files)| {
                    let title = folder.file_name().map_or_else(
                        || folder.display().to_string(),
                        |n| n.to_string_lossy().into_owned(),
                    );
                    (Some((title, None)), files)
                })
                .collect()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use project::{Command, Fingerprint, MediaInfo, MediaKind, MediaLabel, Time};

    fn media(id: u64, path: &str, kind: MediaKind) -> MediaRef {
        MediaRef {
            id: MediaId(id),
            path: path.into(),
            fingerprint: Fingerprint {
                size: id,
                sample_hash: String::new(),
            },
            kind,
            duration: Time::seconds(2),
            info: MediaInfo::default(),
        }
    }

    fn label(p: &Project, name: &str, color: LabelColor, on: &[u64]) -> Project {
        Command::AddMediaLabel {
            label: MediaLabel {
                id: MediaLabelId(0),
                name: name.into(),
                color,
            },
            media: on.iter().map(|i| MediaId(*i)).collect(),
        }
        .apply(p)
        .unwrap()
    }

    /// Five video files and one sound file; Drone on 1 and 2, Interview on 2 and 3.
    fn project() -> Project {
        let mut p = Project::new_automatic("T");
        for (id, path) in [
            (1, "/a/one.mp4"),
            (2, "/b/two.mp4"),
            (3, "/a/three.mp4"),
            (4, "/b/four.mp4"),
            (5, "/a/five.mp4"),
        ] {
            p.media.insert(MediaId(id), media(id, path, MediaKind::Video));
        }
        p.media
            .insert(MediaId(6), media(6, "/a/song.wav", MediaKind::Audio));
        let p = label(&p, "Drone", LabelColor::Green, &[1, 2]);
        label(&p, "Interview", LabelColor::Orange, &[2, 3])
    }

    const SIZES: Sizes = Sizes {
        columns: 2,
        row: 100.0,
        heading: 30.0,
    };

    fn ids(list: &[u64]) -> Vec<MediaId> {
        list.iter().map(|i| MediaId(*i)).collect()
    }

    #[test]
    fn files_go_under_their_first_label_and_the_rest_last() {
        let p = project();
        let a = arrange(&ids(&[5, 4, 3, 2, 1]), &p, GroupBy::Label, SIZES);
        // Drone: 2, 1 (in the order found); Interview: 3 (2 is under Drone);
        // No label: 5, 4.
        assert_eq!(a.items, ids(&[2, 1, 3, 5, 4]));
        let headings: Vec<(&str, usize)> = a
            .lines
            .iter()
            .filter_map(|l| match l {
                Line::Heading { title, count, .. } => Some((title.as_str(), *count)),
                Line::Files { .. } => None,
            })
            .collect();
        assert_eq!(headings, [("Drone", 2), ("Interview", 1), (NO_LABEL, 2)]);
        // Heading, row, heading, row, heading, row.
        assert_eq!(a.lines.len(), 6);
        assert_eq!(a.tops, [0.0, 30.0, 130.0, 160.0, 260.0, 290.0]);
        assert_eq!(a.height, 390.0);
    }

    #[test]
    fn rows_hold_as_many_files_as_there_are_columns() {
        let p = project();
        let a = arrange(&ids(&[1, 2, 3, 4, 5]), &p, GroupBy::Nothing, SIZES);
        assert_eq!(
            a.lines,
            [
                Line::Files { from: 0, to: 2 },
                Line::Files { from: 2, to: 4 },
                Line::Files { from: 4, to: 5 }
            ]
        );
        assert_eq!(a.height, 300.0);
        let gone = arrange(&ids(&[1, 99]), &p, GroupBy::Nothing, SIZES);
        assert_eq!(
            gone.items,
            ids(&[1]),
            "a file no longer in the project is left out"
        );
    }

    #[test]
    fn folders_group_by_where_the_files_are() {
        let p = project();
        let a = arrange(&ids(&[1, 2, 3, 4]), &p, GroupBy::Folder, SIZES);
        assert_eq!(a.items, ids(&[1, 3, 2, 4]));
        assert!(matches!(&a.lines[0], Line::Heading { title, count: 2, .. } if title == "a"));
    }

    #[test]
    fn only_the_lines_in_view_are_drawn() {
        let p = project();
        let found: Vec<MediaId> = (1..=5).map(MediaId).collect();
        let sizes = Sizes { columns: 1, ..SIZES };
        let a = arrange(&found, &p, GroupBy::Nothing, sizes);
        assert_eq!(a.in_view(0.0, 150.0, sizes), 0..2);
        assert_eq!(a.in_view(100.0, 250.0, sizes), 1..3);
        assert_eq!(a.in_view(120.0, 130.0, sizes), 1..2);
        assert_eq!(a.in_view(450.0, 900.0, sizes), 4..5);
        assert_eq!(a.in_view(600.0, 900.0, sizes), 5..5, "past the end: nothing");
        let empty = Arranged::default();
        assert_eq!(empty.in_view(0.0, 100.0, sizes), 0..0);
    }

    #[test]
    fn ten_thousand_files_are_arranged_quickly() {
        let mut p = project();
        for id in 10..10_010 {
            p.media
                .insert(MediaId(id), media(id, &format!("/r/{id}.mp4"), MediaKind::Video));
        }
        let all: Vec<u64> = (10..10_010).collect();
        let p = label(&p, "Many", LabelColor::Blue, &all[..5_000]);
        let found: Vec<MediaId> = p.media.keys().copied().collect();
        let started = std::time::Instant::now();
        let a = arrange(&found, &p, GroupBy::Label, SIZES);
        let facts = Facts::of(&p, Tab::Type(Type::Video), &|_| false, &HashSet::new());
        let took = started.elapsed();
        assert_eq!(a.items.len(), found.len());
        assert_eq!(facts.labels.values().copied().max(), Some(5_000));
        // Done again only when something changed; generous for debug builds.
        assert!(took.as_millis() < 500, "took {took:?}");
    }

    #[test]
    fn the_sidebar_counts_the_open_tab() {
        let mut p = project();
        let sequence = p.active_sequence;
        p = Command::PlaceMedia {
            sequence,
            media: MediaId(1),
        }
        .apply(&p)
        .unwrap_or(p);
        if let Some(m) = p.media.get_mut(&MediaId(4)) {
            m.info.stated_length = Some(Time::seconds(9));
        }
        let needs = |m: MediaId| m == MediaId(5);
        let missing: HashSet<MediaId> = [MediaId(3)].into();
        let facts = Facts::of(&p, Tab::Type(Type::Video), &needs, &missing);
        assert_eq!(facts.in_tab, 5);
        assert_eq!(facts.types.get(&Type::Audio), Some(&1));
        assert_eq!(facts.labels.get(&p.library.labels[0].id), Some(&2));
        assert_eq!(facts.smart.get(&Smart::NeedsProxy), Some(&1));
        assert_eq!(facts.smart.get(&Smart::EndMissing), Some(&1));
        assert_eq!(facts.smart.get(&Smart::Missing), Some(&1));
        let audio = Facts::of(&p, Tab::Type(Type::Audio), &needs, &missing);
        assert_eq!(audio.in_tab, 1);
        assert_eq!(audio.smart.get(&Smart::NotUsed), Some(&1));
    }
}
