//! The user guide: a web page installed beside the programs
//! (`docs/user-guide/` in the source tree), opened in the system's web
//! browser from Settings (the Help tab, the same row in the editor and the
//! launcher), from the editor's search (Ctrl+K "help") and with its key
//! (F1 unless changed).
//!
//! Where it is looked for, in order ([`places`]):
//!
//! 1. `user-guide/index.html` beside the program: each installed version's
//!    folder on Windows (`versions/<version>/`), where the editor and a copy
//!    of the launcher live;
//! 2. any place the program adds: the installed launcher, which sits above
//!    the versions, adds the current version's folder;
//! 3. `../share/user-guide/index.html` from the program's folder: the
//!    AppImage's `usr/bin` and `usr/share`;
//! 4. in a debug build, the source tree's `docs/user-guide/`, so
//!    `cargo run` finds it.
//!
//! Finding it is a few file checks; starting the browser runs on a thread
//! of its own, and a failure comes back through [`UserGuide::poll`].

use crate::settings::{Row, Section, Setting};
use diag::AppError;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, TryRecvError, channel};

/// The folder the guide is installed in, beside the programs.
pub const FOLDER: &str = "user-guide";
/// The page to open in it.
pub const PAGE: &str = "index.html";

/// The id of the Settings row that opens the guide.
pub const ROW: &str = "user-guide";

/// The Help card of Settings, the same in every program.
pub const SETTINGS: Section = Section {
    id: "help-section",
    title: "Help",
    settings: &[Setting::new(
        ROW,
        "User guide",
        "Every feature explained step by step, with pictures and every shortcut. Opens in your web browser.",
    )
    .keywords("help guide manual docs documentation how to tutorial instructions f1")],
};

/// Where the guide may be, most likely first, for a program in
/// `program_dir`, with `also` (a folder holding the guide's folder) tried
/// after the program's own.
pub fn places(program_dir: &Path, also: Option<&Path>) -> Vec<PathBuf> {
    let mut places = vec![program_dir.join(FOLDER).join(PAGE)];
    if let Some(also) = also {
        places.push(also.join(FOLDER).join(PAGE));
    }
    if let Some(above) = program_dir.parent() {
        places.push(above.join("share").join(FOLDER).join(PAGE));
    }
    if cfg!(debug_assertions) {
        places.push(source_tree());
    }
    places
}

/// The guide in the source tree, for programs run with `cargo run`.
fn source_tree() -> PathBuf {
    let crate_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    // crates/ui-kit, two folders under the root of the source tree.
    let root = crate_dir.ancestors().nth(2).unwrap_or(crate_dir);
    root.join("docs").join(FOLDER).join(PAGE)
}

/// The first of `places` that is there, or why there is none.
pub fn find_in(places: &[PathBuf]) -> diag::Result<PathBuf> {
    places.iter().find(|p| p.is_file()).cloned().ok_or_else(|| {
        let looked = places
            .iter()
            .map(|p| p.display().to_string())
            .collect::<Vec<_>>()
            .join(", ");
        AppError::new("The user guide can't be found")
            .why(format!(
                "It is installed with the app, but it isn't in any of the places it should be: {looked}."
            ))
            .fix("Install the app again (your projects and settings stay as they are); the guide comes with it.")
    })
}

/// The program's own folder, for [`places`].
pub fn program_dir() -> diag::Result<PathBuf> {
    let exe = std::env::current_exe().map_err(|err| {
        AppError::new("The user guide can't be found")
            .why("The program could not tell which folder it was started from.")
            .fix("Install the app again; the guide comes with it.")
            .source(err)
    })?;
    Ok(exe.parent().map_or_else(|| PathBuf::from("."), Path::to_path_buf))
}

/// Opens the guide in the web browser and reports what went wrong. One per
/// window; [`UserGuide::poll`] every frame.
#[derive(Debug, Default)]
pub struct UserGuide {
    /// A folder to look in after the program's own (see [`places`]).
    also: Option<PathBuf>,
    /// The browser being started: its answer.
    starting: Option<Receiver<diag::Result<()>>>,
    /// Where it was found, worked out once for the Settings row.
    found: Option<Option<PathBuf>>,
}

impl UserGuide {
    /// A guide also looked for in `also` (a folder holding `user-guide/`).
    pub fn also_in(also: Option<PathBuf>) -> UserGuide {
        UserGuide {
            also,
            ..UserGuide::default()
        }
    }

    /// Where it is looked for.
    pub fn places(&self) -> diag::Result<Vec<PathBuf>> {
        Ok(places(&program_dir()?, self.also.as_deref()))
    }

    /// Where it is, if it is there.
    pub fn find(&self) -> diag::Result<PathBuf> {
        find_in(&self.places()?)
    }

    /// Opens the guide in the system's web browser. The browser is started
    /// on a thread of its own; an error now means the guide isn't there,
    /// and a problem starting the browser comes from [`UserGuide::poll`].
    pub fn open(&mut self, ctx: &egui::Context) -> diag::Result<PathBuf> {
        let page = self.find()?;
        let (tell, answer) = channel();
        let ctx = ctx.clone();
        let opening = page.clone();
        std::thread::Builder::new()
            .name("user-guide".into())
            .spawn(move || {
                let result = os::open_with_system(&opening).map_err(|err| {
                    AppError::new("The user guide can't be opened")
                        .why(format!(
                            "The system has no web browser to open {} with, or it didn't start.",
                            opening.display()
                        ))
                        .fix(format!(
                            "Open {} in your web browser yourself (drag it onto a browser window), or set a default web browser in the system's settings.",
                            opening.display()
                        ))
                        .source(err)
                });
                // The window may be gone by now; nothing to tell then.
                let _ = tell.send(result);
                ctx.request_repaint();
            })
            .map_err(|err| {
                AppError::new("The user guide can't be opened")
                    .why("The program could not start a thread to open the web browser.")
                    .fix(format!("Open {} in your web browser yourself.", page.display()))
                    .source(err)
            })?;
        self.starting = Some(answer);
        tracing::info!(page = %page.display(), "opening the user guide");
        Ok(page)
    }

    /// A problem starting the browser, once it is known. Never waits.
    pub fn poll(&mut self) -> Option<AppError> {
        let answer = self.starting.as_ref()?.try_recv();
        match answer {
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => {
                self.starting = None;
                None
            }
            Ok(result) => {
                self.starting = None;
                result.err()
            }
        }
    }

    /// Draws the control of the Help card's row (other rows are left
    /// alone): an Open button, with where the guide is under it. Returns a
    /// problem opening it.
    pub fn settings_row(&mut self, row: &mut Row<'_>) -> Option<AppError> {
        if row.id() != ROW {
            return None;
        }
        // A few file checks, once per Settings window rather than per frame.
        if self.found.is_none() {
            self.found = Some(self.places().ok().and_then(|p| find_in(&p).ok()));
        }
        let found = self.found.clone().flatten();
        let clicked = crate::settings::action(row.ui, "Open").clicked();
        match &found {
            Some(page) => row.note(page.display().to_string()),
            None => row.note("Not found beside the program: install the app again to get it."),
        }
        if clicked {
            let ctx = row.ui.ctx().clone();
            // Looked for again: it may have come or gone since.
            self.found = None;
            return self.open(&ctx).err();
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_guide_is_looked_for_beside_the_program_then_where_the_program_says_then_in_share() {
        let program = Path::new("/apps/editor/versions/0.4.0");
        let also = Path::new("/apps/editor/versions/0.4.0");
        let found = places(program, Some(also));
        assert_eq!(found[0], program.join("user-guide/index.html"));
        assert_eq!(found[1], also.join("user-guide/index.html"));
        assert_eq!(
            found[2],
            Path::new("/apps/editor/versions/share/user-guide/index.html")
        );
        // The AppImage: usr/bin beside usr/share.
        let appimage = places(Path::new("/tmp/.mount_x/usr/bin"), None);
        assert_eq!(
            appimage[1],
            Path::new("/tmp/.mount_x/usr/share/user-guide/index.html")
        );
    }

    #[test]
    fn a_debug_build_finds_the_guide_in_the_source_tree() {
        let nowhere = tempfile::tempdir().unwrap();
        let found = find_in(&places(nowhere.path(), None)).unwrap();
        assert!(
            found.ends_with("docs/user-guide/index.html"),
            "{}",
            found.display()
        );
        assert!(found.is_file());
    }

    #[test]
    fn the_first_place_that_has_it_wins() {
        let dir = tempfile::tempdir().unwrap();
        let program = dir.path().join("versions/1.0.0");
        let installed = program.join(FOLDER);
        std::fs::create_dir_all(&installed).unwrap();
        std::fs::write(installed.join(PAGE), "<html></html>").unwrap();
        assert_eq!(find_in(&places(&program, None)).unwrap(), installed.join(PAGE));

        // The launcher above the versions finds it through the version it adds.
        let found = find_in(&places(dir.path(), Some(&program))).unwrap();
        assert_eq!(found, installed.join(PAGE));
    }

    #[test]
    fn a_missing_guide_is_explained_with_where_it_was_looked_for_and_a_fix() {
        let dir = tempfile::tempdir().unwrap();
        let err = find_in(&[dir.path().join(FOLDER).join(PAGE)]).unwrap_err();
        assert_eq!(err.what(), "The user guide can't be found");
        assert!(
            err.why_text()
                .unwrap()
                .contains(&dir.path().display().to_string())
        );
        assert!(err.fix_text().contains("Install the app again"));
    }

    #[test]
    fn settings_find_the_guide_by_the_words_people_use() {
        let help = crate::settings::Category {
            id: "help",
            label: "Help",
            sections: &[SETTINGS],
        };
        for words in ["help", "guide", "manual", "docs", "documentation", "user guide"] {
            let hits = crate::settings::search(std::slice::from_ref(&help), words);
            assert!(
                hits.iter().any(|p| p
                    .setting(std::slice::from_ref(&help))
                    .is_some_and(|s| s.id == ROW)),
                "{words}"
            );
        }
    }
}
