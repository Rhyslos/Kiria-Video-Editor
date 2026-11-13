//! "Save this frame as a picture": asks where, then writes the frame at the
//! playhead at its file's full size on a worker.

use jobs::{JobHandle, JobSystem, PoolKind, Priority};
use project::{Project, Time};
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, channel};

#[derive(Default)]
pub struct FrameGrab {
    /// The save dialog, with the project and time it was asked for.
    dialog: Option<(Receiver<Option<PathBuf>>, Project, Time)>,
    saving: Option<(PathBuf, JobHandle<()>)>,
}

/// What came of a save.
pub enum Saved {
    Done(PathBuf),
    Failed(diag::AppError),
}

impl FrameGrab {
    /// Asks where to save the frame at `at` (the dialog runs on its own thread).
    pub fn start(&mut self, ctx: &egui::Context, project: &Project, at: Time, default_name: String) {
        if self.dialog.is_some() || self.saving.is_some() {
            return;
        }
        let (tx, rx) = channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let picked = ui_kit::file_dialog::run(&ctx, || {
                rfd::FileDialog::new()
                    .set_title("Save this frame as a picture")
                    .set_file_name(default_name)
                    .add_filter("PNG picture", &["png"])
                    .save_file()
            });
            let _ = tx.send(picked);
            ctx.request_repaint();
        });
        self.dialog = Some((rx, project.clone(), at));
    }

    /// Starts the save once a file is chosen; returns how it went when done.
    pub fn poll(&mut self, jobs: &JobSystem, ctx: &egui::Context) -> Option<Saved> {
        if let Some((rx, _, _)) = &self.dialog
            && let Ok(picked) = rx.try_recv()
            && let Some((_, project, at)) = self.dialog.take()
            && let Some(path) = picked
        {
            let path = with_png(path);
            let (out, ctx) = (path.clone(), ctx.clone());
            let handle =
                jobs.submit_write(PoolKind::Export, Priority::Interactive, "save frame", move |_| {
                    let result = engine::thumbnail::save_frame(&project, project.active_sequence, at, &out);
                    ctx.request_repaint();
                    result
                });
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

/// The chosen file name, ending in `.png`.
fn with_png(path: PathBuf) -> PathBuf {
    if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("png")) {
        path
    } else {
        let mut name = path.into_os_string();
        name.push(".png");
        PathBuf::from(name)
    }
}

/// "My project 00-01-23-15.png": the project and the frame's timecode.
pub fn default_name(project: &str, timecode: &str) -> String {
    format!("{project} {}.png", timecode.replace(':', "-"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_end_in_png() {
        assert_eq!(with_png("a/b".into()), PathBuf::from("a/b.png"));
        assert_eq!(with_png("a/b.PNG".into()), PathBuf::from("a/b.PNG"));
        assert_eq!(default_name("Clip", "00:01:23:15"), "Clip 00-01-23-15.png");
    }
}
