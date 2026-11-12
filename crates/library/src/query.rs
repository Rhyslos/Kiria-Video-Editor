//! What a search asks for: words, types, labels, a list, whether the files
//! are used, and an order.

use project::{MediaKind, MediaLabelId, MediaListId};

/// The kinds of things the library holds. Effects come with the effects
/// library; until then nothing is of that type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Type {
    Video,
    Audio,
    Image,
    Effect,
}

impl Type {
    pub const ALL: [Type; 4] = [Type::Video, Type::Audio, Type::Image, Type::Effect];

    /// Its number in the `types` table.
    pub fn code(self) -> i64 {
        match self {
            Type::Video => 1,
            Type::Audio => 2,
            Type::Image => 3,
            Type::Effect => 4,
        }
    }

    /// The name shown on its tab.
    pub fn name(self) -> &'static str {
        match self {
            Type::Video => "Video",
            Type::Audio => "Audio",
            Type::Image => "Images",
            Type::Effect => "Effects",
        }
    }
}

impl From<MediaKind> for Type {
    fn from(kind: MediaKind) -> Type {
        match kind {
            MediaKind::Video => Type::Video,
            MediaKind::Audio => Type::Audio,
            MediaKind::Image => Type::Image,
        }
    }
}

/// Whether files are used on a timeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Usage {
    #[default]
    Any,
    /// On at least one timeline of the project.
    Used,
    /// On none.
    Unused,
}

/// The order results come in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Sort {
    /// By name, whatever the capitals.
    #[default]
    Name,
    /// Imported last first.
    Newest,
    Oldest,
    Longest,
    Shortest,
    /// Video, then sound, then pictures; by name within each.
    Type,
    /// Most clips on the timelines first.
    MostUsed,
    /// In a list's own order (by name when no list is asked for).
    ListOrder,
}

impl Sort {
    pub const ALL: [Sort; 8] = [
        Sort::Name,
        Sort::Newest,
        Sort::Oldest,
        Sort::Longest,
        Sort::Shortest,
        Sort::Type,
        Sort::MostUsed,
        Sort::ListOrder,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Sort::Name => "Name",
            Sort::Newest => "Newest first",
            Sort::Oldest => "Oldest first",
            Sort::Longest => "Longest first",
            Sort::Shortest => "Shortest first",
            Sort::Type => "Type",
            Sort::MostUsed => "Most used",
            Sort::ListOrder => "List order",
        }
    }
}

/// One search. The default finds everything, by name.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub struct Query {
    /// Typed words; matched in any case, anywhere in a file's name, its
    /// labels' names and its note.
    pub text: String,
    /// Only these types; empty for all.
    pub types: Vec<Type>,
    /// Only files with every one of these labels.
    pub labels: Vec<MediaLabelId>,
    /// Only files in this list.
    pub list: Option<MediaListId>,
    pub usage: Usage,
    pub sort: Sort,
}

impl Query {
    /// A query for these words and nothing else.
    pub fn text(text: impl Into<String>) -> Query {
        Query {
            text: text.into(),
            ..Query::default()
        }
    }

    /// The typed words, lower case, without empty ones.
    pub fn words(&self) -> Vec<String> {
        self.text.split_whitespace().map(str::to_lowercase).collect()
    }
}
