//! The printable cheat sheet (the keyboard shortcuts page's "Cheat
//! sheet…"): asks where, then makes the page from the shortcut table as it
//! is at that moment and writes it on a worker.

use diag::{AppError, Explain};
use jobs::{JobHandle, JobSystem, PoolKind, Priority};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, channel};
use ui_kit::ShortcutTable;
use ui_kit::keyboard::LayoutId;

#[derive(Default)]
pub struct CheatSheet {
    /// The save dialog, with the table and keyboard it was asked for.
    dialog: Option<(Receiver<Option<PathBuf>>, ShortcutTable, LayoutId)>,
    saving: Option<(PathBuf, JobHandle<()>)>,
}

/// What came of a save.
pub enum Saved {
    Done(PathBuf),
    Failed(AppError),
}

impl CheatSheet {
    /// Asks where to save the sheet (the dialog runs on its own thread).
    pub fn start(&mut self, ctx: &egui::Context, table: &ShortcutTable, layout: LayoutId) {
        if self.dialog.is_some() || self.saving.is_some() {
            return;
        }
        let (tx, rx) = channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let picked = rfd::FileDialog::new()
                .set_title("Save the cheat sheet")
                .set_file_name(default_name())
                .add_filter("Web page", &["html"])
                .save_file();
            let _ = tx.send(picked);
            ctx.request_repaint();
        });
        self.dialog = Some((rx, table.clone(), layout));
    }

    /// Starts the save once a file is chosen; returns how it went when done.
    pub fn poll(&mut self, jobs: &JobSystem, ctx: &egui::Context) -> Option<Saved> {
        if let Some((rx, _, _)) = &self.dialog
            && let Ok(picked) = rx.try_recv()
            && let Some((_, table, layout)) = self.dialog.take()
            && let Some(path) = picked
        {
            let path = with_html(path);
            let (out, ctx) = (path.clone(), ctx.clone());
            let handle = jobs.submit(
                PoolKind::Export,
                Priority::Interactive,
                "cheat sheet",
                move |_| {
                    let result = write(&out, &table, layout);
                    ctx.request_repaint();
                    result
                },
            );
            self.saving = Some((path, handle));
        }
        let (_, handle) = self.saving.as_ref()?;
        let result = handle.try_result()?;
        let (path, _) = self.saving.take()?;
        Some(match result {
            Ok(()) => Saved::Done(path),
            Err(err) => Saved::Failed(err),
        })
    }
}

/// Makes the sheet for `table` on the `layout` keyboard and writes it to `path`.
pub fn write(path: &Path, table: &ShortcutTable, layout: LayoutId) -> diag::Result<()> {
    let page = ui_kit::keyboard::cheat_sheet::html(table, layout.layout(), layout.label(), brand::APP_NAME);
    std::fs::write(path, page).or_explain(|| {
        AppError::new("The cheat sheet could not be saved")
            .why(format!("Writing {} failed.", path.display()))
            .fix("Pick another folder, or check that the disk is not full.")
    })
}

/// The chosen file name, ending in `.html`.
fn with_html(path: PathBuf) -> PathBuf {
    let is_page = path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("html") || e.eq_ignore_ascii_case("htm"));
    if is_page {
        path
    } else {
        let mut name = path.into_os_string();
        name.push(".html");
        PathBuf::from(name)
    }
}

/// "<app> keyboard shortcuts.html".
fn default_name() -> String {
    format!("{} keyboard shortcuts.html", brand::APP_NAME)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ui_kit::{Action, Shortcut};

    #[test]
    fn names_end_in_html() {
        assert_eq!(with_html("a/b".into()), PathBuf::from("a/b.html"));
        assert_eq!(with_html("a/b.HTML".into()), PathBuf::from("a/b.HTML"));
        assert_eq!(with_html("a/b.htm".into()), PathBuf::from("a/b.htm"));
        assert_eq!(with_html("a/b.txt".into()), PathBuf::from("a/b.txt.html"));
        assert!(default_name().ends_with(" keyboard shortcuts.html"));
    }

    #[test]
    fn the_written_sheet_has_the_shortcuts_in_use_and_the_app_s_name() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("sheet.html");
        let mut table = ShortcutTable::default();
        let split = table
            .lines()
            .iter()
            .position(|b| b.action == Action::Split)
            .unwrap();
        table.swap(split, Shortcut::ctrl(egui::Key::B));
        write(&path, &table, LayoutId::Uk).unwrap();
        let page = std::fs::read_to_string(&path).unwrap();
        assert!(page.contains(&format!("<h1>{} keyboard shortcuts</h1>", brand::APP_NAME)));
        assert!(page.contains("<td class=\"keys\">Ctrl+B</td><td>Split at the edit needle</td>"));
        assert!(page.contains("Keyboard: UK English."));
        assert!(page.contains("<h2>Mouse</h2>"));
    }

    #[test]
    fn a_sheet_that_cannot_be_written_says_why() {
        let tmp = tempfile::tempdir().unwrap();
        let nowhere = tmp.path().join("no such folder").join("sheet.html");
        let err = write(&nowhere, &ShortcutTable::default(), LayoutId::Us).unwrap_err();
        assert!(err.report().contains("The cheat sheet could not be saved"));
    }
}
