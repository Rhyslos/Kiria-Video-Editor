//! The viewer in a window of its own, to drag to another screen and fill it.
//!
//! The window is a second egui viewport, shown deferred: egui draws it in a
//! pass of its own, after the main window's, so drawing and presenting it
//! never adds to the main window's interface time. Every window of the app
//! shares one painter in egui-wgpu, so it draws on the same graphics device
//! with the same queue and the same paint-callback resources: the picture
//! the main viewer has on show (a finished frame on the card, decoded
//! planes, or a texture) is drawn here as it is, with no copy.
//!
//! A deferred viewport's drawing can't borrow the editor, so the two share
//! a small [`Shared`] behind a lock (both run on the event loop's thread,
//! one after the other, so the lock never waits): the main window puts in
//! what is on show, and the window puts back what happened in it (keys
//! pressed, the size its picture is drawn at, where the window is, that it
//! was closed).
//!
//! In the window, the full-screen command's keys (F) or a double-click on
//! the picture fill the screen it is on, and Escape gives the screen back;
//! other commands' keys work as in the main window. Closing it puts the
//! viewer back in the main window.

use crate::viewer::OnShow;
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};
use ui_kit::{Action, ShortcutTable, Theme};

/// Where the window was and how big, in points, so it opens there again.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct WindowPlace {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl WindowPlace {
    /// The usual size of a window opened for the first time.
    pub const FIRST: (f32, f32) = (960.0, 540.0);
    /// The smallest it is made, whatever was kept.
    const LEAST: (f32, f32) = (320.0, 180.0);

    /// The window's place from where its outside and its inside are now.
    pub fn of(outer: egui::Rect, inner: egui::Rect) -> WindowPlace {
        WindowPlace {
            x: outer.min.x,
            y: outer.min.y,
            width: inner.width(),
            height: inner.height(),
        }
    }

    /// How the window is opened: where it was kept, or in the usual size
    /// where the system puts it.
    pub fn builder(place: Option<WindowPlace>) -> egui::ViewportBuilder {
        let builder = ui_kit::icon::with_window_icon(
            egui::ViewportBuilder::default()
                .with_title(format!("{} · Viewer", brand::APP_NAME))
                // On top of the main window. On Windows a window only gets
                // its turn to draw while no window above it wants to: the
                // main window, which draws every frame while playing, would
                // otherwise keep this one on its first picture.
                .with_window_level(egui::WindowLevel::AlwaysOnTop)
                .with_min_inner_size(WindowPlace::LEAST),
        );
        match place {
            Some(place) => builder
                .with_position(egui::pos2(place.x, place.y))
                .with_inner_size(
                    egui::vec2(place.width, place.height)
                        .max(egui::vec2(WindowPlace::LEAST.0, WindowPlace::LEAST.1)),
                ),
            None => builder.with_inner_size(WindowPlace::FIRST),
        }
    }
}

/// What the main window and the viewer's window share.
struct Shared {
    on_show: OnShow,
    theme: Theme,
    shortcuts: ShortcutTable,
    /// The main window asked for full screen to be switched (its keys
    /// pressed there, or the Display menu).
    switch_full_screen: bool,
    /// Commands whose keys were pressed in the window, for the main window to run.
    actions: Vec<Action>,
    /// The picture's size in the window, in pixels.
    picture_px: Option<(u32, u32)>,
    place: Option<WindowPlace>,
    full_screen: bool,
    closed: bool,
    /// The picture on show was drawn since the main window last asked.
    drawn: bool,
}

/// What happened in the window since the main window last looked.
#[derive(Debug, Default, PartialEq)]
pub struct News {
    pub actions: Vec<Action>,
    /// Closed with its close button: the viewer goes back.
    pub closed: bool,
    /// The picture on show was drawn.
    pub drawn: bool,
}

/// The viewer's own window, while it is open.
pub struct ViewerWindow {
    shared: Arc<Mutex<Shared>>,
    builder: egui::ViewportBuilder,
}

impl ViewerWindow {
    pub fn id() -> egui::ViewportId {
        egui::ViewportId::from_hash_of("viewer-window")
    }

    /// Opens at `place` (kept from last time) or in the usual size.
    pub fn open(place: Option<WindowPlace>, theme: Theme, shortcuts: &ShortcutTable) -> ViewerWindow {
        ViewerWindow {
            shared: Arc::new(Mutex::new(Shared {
                on_show: OnShow::default(),
                theme,
                shortcuts: shortcuts.clone(),
                switch_full_screen: false,
                actions: Vec::new(),
                picture_px: None,
                place,
                full_screen: false,
                closed: false,
                drawn: false,
            })),
            builder: WindowPlace::builder(place),
        }
    }

    /// Every frame of the main window: keeps the window open (a viewport
    /// that isn't declared in a frame closes), hands it the picture on show
    /// and asks it to draw again when that changed, and takes back what
    /// happened there.
    pub fn show(
        &self,
        ctx: &egui::Context,
        on_show: OnShow,
        theme: Theme,
        shortcuts: &ShortcutTable,
    ) -> News {
        let id = ViewerWindow::id();
        let mut news = News::default();
        if let Ok(mut shared) = self.shared.lock() {
            let new_picture = !shared.on_show.same(&on_show);
            let restyled = shared.theme != theme || shared.shortcuts != *shortcuts;
            if new_picture {
                shared.on_show = on_show;
            }
            if restyled {
                shared.theme = theme;
                shared.shortcuts = shortcuts.clone();
            }
            if new_picture || restyled || shared.switch_full_screen {
                ctx.request_repaint_of(id);
            }
            news = News {
                actions: std::mem::take(&mut shared.actions),
                closed: shared.closed,
                drawn: std::mem::take(&mut shared.drawn),
            };
        }
        let shared = Arc::clone(&self.shared);
        ctx.show_viewport_deferred(id, self.builder.clone(), move |ui, class| {
            // Both windows draw on the event loop's thread: this never waits.
            if let Ok(mut shared) = shared.lock() {
                shared.draw(ui, class);
            }
        });
        news
    }

    /// Fill the screen the window is on, or give it back.
    pub fn switch_full_screen(&self) {
        if let Ok(mut shared) = self.shared.lock() {
            shared.switch_full_screen = true;
        }
    }

    pub fn full_screen(&self) -> bool {
        self.shared.lock().is_ok_and(|s| s.full_screen)
    }

    /// The picture's size in the window, in pixels, once it has been drawn.
    pub fn picture_px(&self) -> Option<(u32, u32)> {
        self.shared.lock().ok().and_then(|s| s.picture_px)
    }

    /// Where the window is now (as last drawn, not filling the screen).
    pub fn place(&self) -> Option<WindowPlace> {
        self.shared.lock().ok().and_then(|s| s.place)
    }
}

/// What the window does with a command whose keys were pressed in it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InWindow {
    /// Fill the screen, or give it back.
    SwitchFullScreen,
    /// Give the screen back.
    LeaveFullScreen,
    /// Nothing here.
    Nothing,
    /// The main window runs it.
    Pass(Action),
}

/// What a command pressed in the window does there: the full-screen command
/// is the window's own, Escape gives back the screen it fills, and the
/// others are passed on to the main window as if pressed there.
fn in_window(action: Action, full_screen: bool) -> InWindow {
    match action {
        Action::FullScreenViewer => InWindow::SwitchFullScreen,
        Action::ClearSelection if full_screen => InWindow::LeaveFullScreen,
        Action::ClearSelection => InWindow::Nothing,
        other => InWindow::Pass(other),
    }
}

impl Shared {
    /// One pass of the window: its keys, its picture and where it is.
    fn draw(&mut self, ui: &mut egui::Ui, class: egui::ViewportClass) {
        let ctx = ui.ctx().clone();
        let (closing, full_screen, maximized, inner, outer) = ctx.input(|i| {
            let viewport = i.viewport();
            (
                viewport.close_requested(),
                viewport.fullscreen.unwrap_or(false),
                viewport.maximized.unwrap_or(false),
                viewport.inner_rect,
                viewport.outer_rect,
            )
        });
        self.full_screen = full_screen;
        let mut switch = std::mem::take(&mut self.switch_full_screen);
        let mut leave = false;
        for action in self.shortcuts.pressed(&ctx) {
            match in_window(action, full_screen) {
                InWindow::SwitchFullScreen => switch = !switch,
                InWindow::LeaveFullScreen => leave = true,
                InWindow::Nothing => {}
                InWindow::Pass(action) => self.actions.push(action),
            }
        }
        let theme = self.theme;
        let draw_picture = |ui: &mut egui::Ui, on_show: &OnShow| {
            let size = ui.available_size();
            ui_kit::viewer::viewer(ui, &theme, size, on_show.timeline, on_show.picture(), "")
        };
        let (frame, response) = if class == egui::ViewportClass::EmbeddedWindow {
            // This system can't open another window: egui shows it as a
            // window inside the main one instead.
            ui.set_min_size(egui::vec2(480.0, 270.0));
            draw_picture(ui, &self.on_show)
        } else {
            egui::CentralPanel::default()
                .frame(egui::Frame::new().fill(theme.bg))
                .show(ui, |ui| draw_picture(ui, &self.on_show))
                .inner
        };
        if response.double_clicked() {
            switch = !switch;
        }
        if switch || (leave && full_screen) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(switch && !full_screen));
        }
        let ppp = ctx.pixels_per_point();
        let picture_px = Some(((frame.width() * ppp) as u32, (frame.height() * ppp) as u32));
        let resized = std::mem::replace(&mut self.picture_px, picture_px) != picture_px;
        if !full_screen
            && !maximized
            && let (Some(outer), Some(inner)) = (outer, inner)
        {
            self.place = Some(WindowPlace::of(outer, inner));
        }
        self.drawn = true;
        if closing {
            self.closed = true;
        }
        // The main window has news to take in: keys, the picture's size
        // (the next picture is asked for at it), closing.
        if !self.actions.is_empty() || resized || closing {
            ctx.request_repaint_of(egui::ViewportId::ROOT);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_full_screen_keys_are_the_windows_own_and_the_rest_go_to_the_main_window() {
        assert_eq!(
            in_window(Action::FullScreenViewer, false),
            InWindow::SwitchFullScreen
        );
        assert_eq!(
            in_window(Action::FullScreenViewer, true),
            InWindow::SwitchFullScreen
        );
        assert_eq!(in_window(Action::ClearSelection, true), InWindow::LeaveFullScreen);
        assert_eq!(
            in_window(Action::ClearSelection, false),
            InWindow::Nothing,
            "Escape in the window never clears the timeline's selection"
        );
        assert_eq!(
            in_window(Action::PlayPause, false),
            InWindow::Pass(Action::PlayPause)
        );
        assert_eq!(
            in_window(Action::ViewerWindow, true),
            InWindow::Pass(Action::ViewerWindow)
        );
    }

    #[test]
    fn the_window_opens_where_it_was_and_never_too_small() {
        assert_eq!(
            WindowPlace::builder(None).window_level,
            Some(egui::WindowLevel::AlwaysOnTop),
            "above the main window, so Windows lets it draw"
        );
        let place = WindowPlace {
            x: 1920.0,
            y: 40.0,
            width: 100.0,
            height: 900.0,
        };
        let builder = WindowPlace::builder(Some(place));
        assert_eq!(builder.position, Some(egui::pos2(1920.0, 40.0)));
        assert_eq!(builder.inner_size, Some(egui::vec2(320.0, 900.0)));
        let first = WindowPlace::builder(None);
        assert_eq!(first.position, None, "the system places a first window");
        assert_eq!(first.inner_size, Some(egui::vec2(960.0, 540.0)));
    }

    #[test]
    fn its_place_is_where_its_outside_starts_and_how_big_its_inside_is() {
        let outer = egui::Rect::from_min_size(egui::pos2(100.0, 50.0), egui::vec2(820.0, 640.0));
        let inner = egui::Rect::from_min_size(egui::pos2(110.0, 80.0), egui::vec2(800.0, 600.0));
        assert_eq!(
            WindowPlace::of(outer, inner),
            WindowPlace {
                x: 100.0,
                y: 50.0,
                width: 800.0,
                height: 600.0
            }
        );
    }

    #[test]
    fn the_place_round_trips_as_settings_text() {
        let place = WindowPlace {
            x: -1200.0,
            y: 30.5,
            width: 1280.0,
            height: 720.0,
        };
        let text = ron::to_string(&place).unwrap();
        assert_eq!(ron::from_str::<WindowPlace>(&text).unwrap(), place);
    }
}
