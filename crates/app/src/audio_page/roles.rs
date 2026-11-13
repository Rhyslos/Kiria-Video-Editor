//! Track roles as the page shows them, and the Show filter that narrows the
//! timeline and the mixer to one of them.

use project::TrackRole;
use ui_kit::theme::role;

/// Every role, in the order menus offer them.
pub const ROLES: [TrackRole; 5] = [
    TrackRole::Dialogue,
    TrackRole::Music,
    TrackRole::Effects,
    TrackRole::Ambience,
    TrackRole::Other,
];

/// A role's name.
pub fn name(r: TrackRole) -> &'static str {
    match r {
        TrackRole::Dialogue => "Dialogue",
        TrackRole::Music => "Music",
        TrackRole::Effects => "SFX and foley",
        TrackRole::Ambience => "Ambience",
        TrackRole::Other => "Other",
    }
}

/// A role's colour.
pub fn colour(r: TrackRole) -> egui::Color32 {
    match r {
        TrackRole::Dialogue => role::DIALOGUE,
        TrackRole::Music => role::MUSIC,
        TrackRole::Effects => role::EFFECTS,
        TrackRole::Ambience => role::AMBIENCE,
        TrackRole::Other => role::OTHER,
    }
}

/// The roles as the shared widgets take them: names and colours.
pub fn offered() -> [(&'static str, egui::Color32); 5] {
    ROLES.map(|r| (name(r), colour(r)))
}

/// A role's place in [`ROLES`].
pub fn index(r: TrackRole) -> usize {
    ROLES.iter().position(|x| *x == r).unwrap_or(ROLES.len() - 1)
}

/// The role at `i` in [`ROLES`].
pub fn at(i: usize) -> Option<TrackRole> {
    ROLES.get(i).copied()
}

/// Which tracks the timeline and the mixer show.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Show {
    #[default]
    All,
    Only(TrackRole),
}

impl Show {
    /// Every choice, as the dropdown offers them.
    pub fn all() -> [Show; 5] {
        [
            Show::All,
            Show::Only(TrackRole::Dialogue),
            Show::Only(TrackRole::Music),
            Show::Only(TrackRole::Effects),
            Show::Only(TrackRole::Ambience),
        ]
    }

    pub fn label(self) -> &'static str {
        match self {
            Show::All => "All tracks",
            Show::Only(r) => name(r),
        }
    }

    /// Whether a track with role `r` is shown.
    pub fn shows(self, r: TrackRole) -> bool {
        match self {
            Show::All => true,
            Show::Only(only) => only == r,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_filter_shows_one_role_or_all_and_roles_round_trip() {
        assert!(Show::All.shows(TrackRole::Other));
        assert!(Show::Only(TrackRole::Music).shows(TrackRole::Music));
        assert!(!Show::Only(TrackRole::Music).shows(TrackRole::Dialogue));
        for r in ROLES {
            assert_eq!(at(index(r)), Some(r));
        }
        assert_eq!(Show::Only(TrackRole::Effects).label(), "SFX and foley");
        let names: std::collections::HashSet<_> = ROLES.iter().map(|r| name(*r)).collect();
        assert_eq!(names.len(), ROLES.len());
    }
}
