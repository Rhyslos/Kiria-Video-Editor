//! The pictures on the project cards, written by the editor on every save.
//! Each is read on a worker the first time its card shows, and again when
//! the project file changes.

use crate::work::{Poll, Task};
use project_index::ProjectEntry;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// How many pictures are read at the same time.
const READING_AT_ONCE: usize = 4;

enum State {
    Reading(Task<Option<egui::ColorImage>>),
    Ready(egui::TextureHandle),
    /// No picture yet (or it can't be read): the card shows its settings.
    None,
}

#[derive(Default)]
pub struct Thumbnails {
    /// Per project: the file's change time when read, and the picture.
    cards: HashMap<PathBuf, (Option<i64>, State)>,
}

impl Thumbnails {
    /// The picture for `entry`, once read. Starts reading it otherwise.
    pub fn get(&mut self, ctx: &egui::Context, entry: &ProjectEntry) -> Option<&egui::TextureHandle> {
        let stale = self
            .cards
            .get(&entry.path)
            .is_none_or(|(modified, _)| *modified != entry.modified);
        let reading = self
            .cards
            .values()
            .filter(|(_, s)| matches!(s, State::Reading(_)))
            .count();
        if stale && reading < READING_AT_ONCE {
            let file = project::thumbnail_path(&entry.path);
            let task = Task::spawn(ctx, move || read(&file));
            self.cards
                .insert(entry.path.clone(), (entry.modified, State::Reading(task)));
        }
        let (_, state) = self.cards.get_mut(&entry.path)?;
        if let State::Reading(task) = state {
            match task.poll() {
                Poll::Running => {}
                Poll::Done(Some(image)) => {
                    let name = format!("card-{}", entry.path.display());
                    *state = State::Ready(ctx.load_texture(name, image, egui::TextureOptions::LINEAR));
                }
                Poll::Done(None) | Poll::Lost => *state = State::None,
            }
        }
        match state {
            State::Ready(texture) => Some(texture),
            _ => None,
        }
    }
}

/// Reads and decodes a picture; `None` when there is none.
fn read(file: &Path) -> Option<egui::ColorImage> {
    let image = image::open(file).ok()?.to_rgba8();
    let size = [image.width() as usize, image.height() as usize];
    Some(egui::ColorImage::from_rgba_unmultiplied(size, image.as_raw()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_a_picture_and_none_when_missing() {
        let dir = std::env::temp_dir().join(format!("launcher-thumb-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("a.png");
        image::save_buffer(
            &file,
            &[10u8, 20, 30, 255].repeat(6),
            3,
            2,
            image::ExtendedColorType::Rgba8,
        )
        .unwrap();
        let picture = read(&file).unwrap();
        assert_eq!(picture.size, [3, 2]);
        assert!(read(&dir.join("missing.png")).is_none());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
