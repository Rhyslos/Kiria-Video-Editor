//! What Settings holds, as data: categories (the tabs), their sections (the
//! cards) and the settings in them (the rows), each with an id, a label, a
//! one-line description and words to be found by. Search and
//! [`place_of`] (open Settings at a setting) work on this alone, so they
//! are the same in every program and can be tested without a window.

/// One row of Settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Setting {
    /// Unique across every category of a program, such as `"proxies"`.
    pub id: &'static str,
    pub label: &'static str,
    /// One line under the label, in muted text. A row can show other words
    /// while it is drawn ([`super::Row::describe`]); search reads these.
    pub description: &'static str,
    /// Other words it is found by (not shown), such as "dark light colours".
    pub keywords: &'static str,
    /// A change applies the next time the program starts: a small badge on
    /// the row says so.
    pub restart: bool,
}

impl Setting {
    pub const fn new(id: &'static str, label: &'static str, description: &'static str) -> Setting {
        Setting {
            id,
            label,
            description,
            keywords: "",
            restart: false,
        }
    }

    /// The same, also found by `keywords`.
    pub const fn keywords(self, keywords: &'static str) -> Setting {
        Setting { keywords, ..self }
    }

    /// The same, marked as applying from the next start.
    pub const fn restart(self) -> Setting {
        Setting {
            restart: true,
            ..self
        }
    }
}

/// One card of a category: a title and its rows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Section {
    pub id: &'static str,
    pub title: &'static str,
    pub settings: &'static [Setting],
}

/// One tab of Settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Category {
    pub id: &'static str,
    pub label: &'static str,
    pub sections: &'static [Section],
}

/// Where a setting is: indices of its category, its section in it and the
/// row in that.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Place {
    pub category: usize,
    pub section: usize,
    pub setting: usize,
}

impl Place {
    /// The setting at this place.
    pub fn setting(self, categories: &[Category]) -> Option<&Setting> {
        categories
            .get(self.category)?
            .sections
            .get(self.section)?
            .settings
            .get(self.setting)
    }
}

/// Where `id` is. It may name a setting, a section (its first row) or a
/// category (its first row), so other parts can point at any of them.
pub fn place_of(categories: &[Category], id: &str) -> Option<Place> {
    for (c, category) in categories.iter().enumerate() {
        if category.id == id {
            return Some(Place {
                category: c,
                section: 0,
                setting: 0,
            });
        }
        for (s, section) in category.sections.iter().enumerate() {
            if section.id == id {
                return Some(Place {
                    category: c,
                    section: s,
                    setting: 0,
                });
            }
            if let Some(r) = section.settings.iter().position(|setting| setting.id == id) {
                return Some(Place {
                    category: c,
                    section: s,
                    setting: r,
                });
            }
        }
    }
    None
}

/// Whether a setting is found by `query`: every word of it appears in the
/// setting's label, description or keywords, or in its section's title or
/// its category's name, ignoring case and a plural's ending ("proxy"
/// finds "Proxies"). An empty query finds everything.
pub fn found(query: &str, category: &Category, section: &Section, setting: &Setting) -> bool {
    let haystack = [
        setting.label,
        setting.description,
        setting.keywords,
        section.title,
        category.label,
    ]
    .join(" ")
    .to_lowercase();
    query
        .to_lowercase()
        .split_whitespace()
        .all(|word| word_found(&haystack, word))
}

/// Whether `word` (lower case) is in `haystack`, as it is or in its other
/// number: "proxy" and "proxies", "frame" and "frames".
fn word_found(haystack: &str, word: &str) -> bool {
    if haystack.contains(word) {
        return true;
    }
    if let Some(stem) = word.strip_suffix('y') {
        return haystack.contains(&format!("{stem}ies"));
    }
    if let Some(stem) = word.strip_suffix("ies") {
        return haystack.contains(&format!("{stem}y"));
    }
    match word.strip_suffix('s') {
        Some(stem) if stem.len() >= 3 => haystack.contains(stem),
        _ => false,
    }
}

/// Every setting `query` finds, in the order Settings shows them.
pub fn search(categories: &[Category], query: &str) -> Vec<Place> {
    let mut places = Vec::new();
    for (c, category) in categories.iter().enumerate() {
        for (s, section) in category.sections.iter().enumerate() {
            for (r, setting) in section.settings.iter().enumerate() {
                if found(query, category, section, setting) {
                    places.push(Place {
                        category: c,
                        section: s,
                        setting: r,
                    });
                }
            }
        }
    }
    places
}

/// Ids used twice, which would make [`place_of`] ambiguous (a program's
/// test calls this on its own categories).
pub fn repeated_ids(categories: &[Category]) -> Vec<&'static str> {
    let mut seen = std::collections::HashSet::new();
    let mut twice = Vec::new();
    for category in categories {
        let ids = std::iter::once(category.id)
            .chain(category.sections.iter().map(|s| s.id))
            .chain(
                category
                    .sections
                    .iter()
                    .flat_map(|s| s.settings.iter().map(|r| r.id)),
            );
        for id in ids {
            if !seen.insert(id) {
                twice.push(id);
            }
        }
    }
    twice
}

#[cfg(test)]
mod tests {
    use super::*;

    const LOOK: &[Section] = &[Section {
        id: "appearance",
        title: "Appearance",
        settings: &[Setting::new("theme", "Theme", "The colours of every window.").keywords("dark light")],
    }];
    const PLAYBACK: &[Section] = &[
        Section {
            id: "proxies-section",
            title: "Proxies",
            settings: &[
                Setting::new(
                    "proxies",
                    "Proxies",
                    "Smaller copies of recordings for smooth editing.",
                ),
                Setting::new("jump-picture", "While jumping", "What the viewer shows first."),
            ],
        },
        Section {
            id: "quality",
            title: "Quality",
            settings: &[Setting::new("threads", "Worker threads", "How many tasks run at once.").restart()],
        },
    ];
    const CATEGORIES: &[Category] = &[
        Category {
            id: "look",
            label: "Look",
            sections: LOOK,
        },
        Category {
            id: "playback",
            label: "Playback & proxies",
            sections: PLAYBACK,
        },
    ];

    #[test]
    fn a_setting_section_or_category_id_opens_the_right_place() {
        let at = |c, s, r| {
            Some(Place {
                category: c,
                section: s,
                setting: r,
            })
        };
        assert_eq!(place_of(CATEGORIES, "theme"), at(0, 0, 0));
        assert_eq!(place_of(CATEGORIES, "jump-picture"), at(1, 0, 1));
        assert_eq!(place_of(CATEGORIES, "threads"), at(1, 1, 0));
        assert_eq!(
            place_of(CATEGORIES, "quality"),
            at(1, 1, 0),
            "a section: its first row"
        );
        assert_eq!(
            place_of(CATEGORIES, "playback"),
            at(1, 0, 0),
            "a category: its first row"
        );
        assert_eq!(place_of(CATEGORIES, "nothing"), None);
        let threads = place_of(CATEGORIES, "threads").and_then(|p| p.setting(CATEGORIES));
        assert!(threads.is_some_and(|s| s.restart && s.label == "Worker threads"));
    }

    #[test]
    fn search_reads_labels_descriptions_keywords_and_titles() {
        let ids = |query: &str| -> Vec<&str> {
            search(CATEGORIES, query)
                .into_iter()
                .filter_map(|p| p.setting(CATEGORIES).map(|s| s.id))
                .collect()
        };
        assert_eq!(
            ids("proxy"),
            ["proxies", "jump-picture", "threads"],
            "the section's title and the category's name count"
        );
        assert_eq!(ids("PROXIES smooth"), ["proxies"], "every word, any case");
        assert_eq!(ids("dark"), ["theme"], "keywords are found");
        assert_eq!(ids("tasks at once"), ["threads"], "descriptions are found");
        assert_eq!(ids("look"), ["theme"], "the category's name counts");
        assert!(ids("zebra").is_empty());
        assert_eq!(ids("thread"), ["threads"]);
        assert_eq!(ids("themes"), ["theme"], "the other number of a word");
        assert_eq!(ids("quality"), ["threads"]);
        assert_eq!(ids("qualities"), ["threads"]);
        assert_eq!(ids("  ").len(), 4, "nothing typed finds everything");
    }

    #[test]
    fn ids_are_told_apart() {
        assert!(repeated_ids(CATEGORIES).is_empty());
        const TWICE: &[Category] = &[Category {
            id: "theme",
            label: "Look",
            sections: LOOK,
        }];
        assert_eq!(repeated_ids(TWICE), ["theme"]);
    }
}
