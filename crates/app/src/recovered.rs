//! What the editor says when a project it opens had unsaved work from a
//! crash: how much was put back (with a way back to the last save), or
//! that it was set aside and where.
//!
//! Recovery replays the whole autosave journal onto the copy of the project
//! it started from (`project::document`), so recovered changes come back
//! with their undo history: each can be undone. "Open last save" here puts
//! the project back as it was last saved in one step (`Command::Restore`),
//! which undo takes back, so nothing is lost by choosing it.

use diag::{AppError, FixAction};
use project::Opened;

/// The notice's one-click fix: go back to the last save.
pub const OPEN_LAST_SAVE: &str = "project.open-last-save";

/// The notice for how a project was opened, if there is anything to say.
/// `undo` is the Undo key's name from the shortcut table.
pub fn notice(opened: &Opened, undo: &str) -> Option<AppError> {
    match opened {
        Opened::AsSaved => None,
        Opened::Recovered { changes, .. } => {
            let (what, were) = counted(*changes);
            let (they, them) = pronouns(*changes);
            Some(
                AppError::new(format!("{what} that {were} not saved before the app closed came back"))
                    .why(format!(
                        "{they} {were} kept in the project's autosave file and {were} put back and saved."
                    ))
                    .fix(format!(
                        "Keep working, or undo {them} ({undo}). Open last save puts the project back as it was last saved in one step, which {undo} takes back."
                    ))
                    .action(FixAction::new("Open last save", OPEN_LAST_SAVE).declined_with("Keep them")),
            )
        }
        Opened::SetAside { changes, kept } => {
            let (what, were) = counted(*changes);
            let (they, _) = pronouns(*changes);
            let is = if *changes == 1 { "is" } else { "are" };
            let name = kept.file_name().map_or_else(
                || kept.display().to_string(),
                |n| n.to_string_lossy().into_owned(),
            );
            Some(
                AppError::new(format!("{what} that {were} not saved before the app closed {were} set aside"))
                    .why("The project opened as it was last saved, as asked.")
                    .fix(format!(
                        "{they} {is} kept in {name} beside the project, which moves and is deleted with it. The app can't put them back from there yet."
                    )),
            )
        }
    }
}

/// "It" and "it" for one change, "They" and "them one by one" for more.
fn pronouns(changes: usize) -> (&'static str, &'static str) {
    if changes == 1 {
        ("It", "it")
    } else {
        ("They", "them one by one")
    }
}

/// "1 change" and "was", or "3 changes" and "were".
fn counted(changes: usize) -> (String, &'static str) {
    let were = if changes == 1 { "was" } else { "were" };
    (project::document::changes_text(changes), were)
}

#[cfg(test)]
mod tests {
    use super::*;
    use project::{Project, SequenceSettings};
    use std::path::PathBuf;

    #[test]
    fn recovered_work_is_counted_and_the_last_save_offered() {
        let opened = Opened::Recovered {
            changes: 1,
            last_save: Box::new(Project::new("P", SequenceSettings::GAME_CLIP)),
        };
        let notice = notice(&opened, "Ctrl+Z").unwrap();
        assert_eq!(
            notice.what(),
            "1 change that was not saved before the app closed came back"
        );
        assert!(
            notice
                .fix_text()
                .starts_with("Keep working, or undo it (Ctrl+Z).")
        );
        assert_eq!(
            notice.why_text(),
            Some("It was kept in the project's autosave file and was put back and saved.")
        );
        assert_eq!(notice.fix_action().unwrap().command, OPEN_LAST_SAVE);
    }

    #[test]
    fn set_aside_work_says_where_it_is() {
        let opened = Opened::SetAside {
            changes: 3,
            kept: PathBuf::from("/p/big.x.journal.discarded-20261113-090000"),
        };
        let notice = notice(&opened, "Ctrl+Z").unwrap();
        assert_eq!(
            notice.what(),
            "3 changes that were not saved before the app closed were set aside"
        );
        assert!(
            notice
                .fix_text()
                .contains("big.x.journal.discarded-20261113-090000")
        );
        assert!(notice.fix_text().starts_with("They are kept in "));
        assert!(notice.fix_action().is_none());
        assert!(super::notice(&Opened::AsSaved, "Ctrl+Z").is_none());
    }
}
