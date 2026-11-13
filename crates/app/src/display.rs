//! The Display menu's ways of showing the Edit page's picture, besides the
//! full screen the editor handles itself: the split view's Source beside
//! the Program, and the viewer in its own window.
//!
//! The Source is a second [`Viewer`] with a worker of its own, made when
//! split view is turned on and closed when it is turned off, so it costs
//! nothing while unused. It is asked for the selected clip's file alone
//! (`engine::source`), so it shows the clip raw: no look, the file's own
//! shape. The project of that file is made once per file and kept, so the
//! viewer's "same as last time" check holds while the playhead moves.

use crate::viewer::Viewer;
use crate::viewer_window::{News, ViewerWindow, WindowPlace};
use engine::source::{NO_PICTURE_OF_ITS_OWN, Source, file_alone, source, timeline_alone};
use project::{ClipId, MediaRef, Project, Time};
use ui_kit::{PreviewQuality, ShortcutTable, Slot, Theme};

/// The picture slot the Source draws from, beside the viewer's own.
const SOURCE_SLOT: Slot = Slot(1);

/// What the Source half says when there is no clip to show.
pub const SELECT_A_CLIP: &str = "Select a clip to see its own picture here";

#[derive(Default)]
pub struct ViewerDisplay {
    /// The split view's Source, while split view is on.
    source: Option<Viewer>,
    /// The file the Source shows, and the project of it alone.
    alone: Option<(MediaRef, Project)>,
    /// The nested timeline the Source shows, the project it was made from
    /// and that project with the timeline open.
    nested: Option<(project::SequenceId, Project, Project)>,
    /// The viewer's own window, while it is open.
    window: Option<ViewerWindow>,
    /// Seconds per frame while playing, handed to the Source's viewer as
    /// to the Program's, so it keeps up the same way.
    playing: Option<f64>,
}

impl ViewerDisplay {
    /// Split view as settings left it; `viewer` is the Edit page's.
    pub fn new(split: bool, viewer: &Viewer) -> ViewerDisplay {
        let mut display = ViewerDisplay::default();
        display.set_split(split, viewer);
        display
    }

    pub fn split(&self) -> bool {
        self.source.is_some()
    }

    /// Turns split view on (a second worker starts beside `viewer`'s) or off
    /// (its worker ends).
    pub fn set_split(&mut self, on: bool, viewer: &Viewer) {
        match (on, self.source.take()) {
            (true, Some(source)) => self.source = Some(source),
            (true, None) => {
                let mut source = viewer.second(SOURCE_SLOT);
                source.set_playing(self.playing);
                self.source = Some(source);
            }
            (false, Some(mut source)) => {
                source.close();
                self.alone = None;
            }
            (false, None) => {}
        }
    }

    /// Playback started (`Some(seconds per frame)`) or stopped (`None`):
    /// the Source follows the playhead frame by frame, as the Program does.
    pub fn set_playing(&mut self, frame_interval: Option<f64>) {
        self.playing = frame_interval;
        if let Some(source) = &mut self.source {
            source.set_playing(frame_interval);
        }
    }

    /// Settings › Proxies' choice for jumps in a file without a proxy,
    /// for the Source as for the Program.
    pub fn set_key_frame_first(&mut self, on: bool) {
        if let Some(source) = &mut self.source {
            source.set_key_frame_first(on);
        }
    }

    /// What the Display menu shows as on.
    pub fn state(&self, full_screen: bool) -> ui_kit::viewer::Display {
        ui_kit::viewer::Display {
            split: self.split(),
            own_window: self.window.is_some(),
            full_screen: full_screen || self.window.as_ref().is_some_and(ViewerWindow::full_screen),
            // The timeline's lines are the editor's settings; it fills them in.
            ..Default::default()
        }
    }

    /// Takes in the Source's newest picture. Returns an error to show.
    pub fn poll(&mut self, ctx: &egui::Context, quality: PreviewQuality) -> Option<diag::AppError> {
        self.source.as_mut()?.poll(ctx, quality)
    }

    /// Whether the Source's last picture asked for has come back.
    pub fn settled(&self) -> bool {
        self.source.as_ref().is_none_or(Viewer::settled)
    }

    /// Draws the Source in `size`: the clip `selected` picks, raw, at the
    /// playhead's place in it; or a quiet line when nothing is selected.
    #[allow(clippy::too_many_arguments)]
    pub fn source_ui(
        &mut self,
        ui: &mut egui::Ui,
        theme: &Theme,
        size: egui::Vec2,
        project: &Project,
        selected: &[ClipId],
        playhead: Time,
        quality: PreviewQuality,
    ) {
        let shot = project.active().map_or(Source::NothingSelected, |seq| {
            source(project, seq, selected, playhead)
        });
        let Some(viewer) = self.source.as_mut() else {
            ui_kit::viewer::empty(ui, theme, size, SELECT_A_CLIP);
            return;
        };
        match shot {
            Source::NothingSelected => {
                ui_kit::viewer::empty(ui, theme, size, SELECT_A_CLIP);
            }
            Source::Adjustment => {
                ui_kit::viewer::empty(ui, theme, size, NO_PICTURE_OF_ITS_OWN);
            }
            Source::Nested { sequence, at, .. } => {
                // The project with the nested timeline open, made again only
                // when the project changes.
                let fresh = self
                    .nested
                    .as_ref()
                    .is_some_and(|(id, made_from, _)| *id == sequence && made_from == project);
                if !fresh {
                    self.nested =
                        timeline_alone(project, sequence).map(|alone| (sequence, project.clone(), alone));
                }
                match &self.nested {
                    Some((_, _, alone)) => {
                        let name = alone.active().map_or("", |s| s.name.as_str());
                        let caption = format!("Source · {name}");
                        viewer.source_ui(ui, theme, size, alone, at, quality, &caption);
                    }
                    None => {
                        ui_kit::viewer::empty(ui, theme, size, SELECT_A_CLIP);
                    }
                }
            }
            Source::Shot { media, at, .. } => {
                if self.alone.as_ref().is_none_or(|(kept, _)| kept != media) {
                    self.alone = file_alone(media).map(|alone| (media.clone(), alone));
                }
                let Some((_, alone)) = &self.alone else {
                    ui_kit::viewer::empty(ui, theme, size, SELECT_A_CLIP);
                    return;
                };
                let caption = caption(media);
                viewer.source_ui(ui, theme, size, alone, at, quality, &caption);
            }
        }
    }

    pub fn window_open(&self) -> bool {
        self.window.is_some()
    }

    /// Opens the viewer's own window at `place`.
    pub fn open_window(&mut self, place: Option<WindowPlace>, theme: Theme, shortcuts: &ShortcutTable) {
        if self.window.is_none() {
            self.window = Some(ViewerWindow::open(place, theme, shortcuts));
        }
    }

    /// Closes the viewer's own window. Returns where it was, to keep.
    pub fn close_window(&mut self) -> Option<WindowPlace> {
        self.window.take().and_then(|window| window.place())
    }

    /// Where the window is now, while it is open.
    pub fn window_place(&self) -> Option<WindowPlace> {
        self.window.as_ref().and_then(ViewerWindow::place)
    }

    /// Fill the screen the viewer's own window is on, or give it back.
    pub fn switch_window_full_screen(&self) {
        if let Some(window) = &self.window {
            window.switch_full_screen();
        }
    }

    /// While the viewer is in its own window: the size its picture is drawn
    /// at there, in pixels, once known. For `viewer::Stage::away`.
    pub fn away(&self) -> Option<Option<(u32, u32)>> {
        self.window.as_ref().map(ViewerWindow::picture_px)
    }

    /// Every frame of the main window: keeps the viewer's own window open
    /// (when it is), hands it `viewer`'s picture and says what happened there.
    pub fn show_window(
        &mut self,
        ctx: &egui::Context,
        viewer: &mut Viewer,
        theme: Theme,
        shortcuts: &ShortcutTable,
    ) -> News {
        let Some(window) = &self.window else {
            return News::default();
        };
        let news = window.show(ctx, viewer.on_show(), theme, shortcuts);
        if news.drawn {
            viewer.mark_drawn(ctx);
        }
        news
    }

    /// The app is closing: the Source's worker finishes and ends.
    pub fn close(&mut self) {
        if let Some(source) = &mut self.source {
            source.close();
        }
    }
}

/// The Source half's caption: "Source · " and the file's name.
pub fn caption(media: &MediaRef) -> String {
    let name = media.path.file_name().map_or_else(
        || media.path.display().to_string(),
        |n| n.to_string_lossy().into_owned(),
    );
    format!("Source · {name}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_caption_names_the_file_without_its_folder() {
        let mut media = MediaRef {
            id: project::MediaId(0),
            path: "/recordings/match 3.mkv".into(),
            fingerprint: project::Fingerprint {
                size: 1,
                sample_hash: String::new(),
            },
            kind: project::MediaKind::Video,
            duration: Time::seconds(1),
            info: project::MediaInfo {
                container: String::new(),
                video: None,
                audio: Vec::new(),
                stated_length: None,
            },
        };
        assert_eq!(caption(&media), "Source · match 3.mkv");
        media.path = "".into();
        assert_eq!(caption(&media), "Source · ");
    }

    #[test]
    fn the_menu_shows_what_is_on() {
        let display = ViewerDisplay::default();
        assert_eq!(display.state(false), ui_kit::viewer::Display::default());
        assert!(display.state(true).full_screen);
        assert_eq!(display.away(), None, "the picture is in the main window");
    }
}
