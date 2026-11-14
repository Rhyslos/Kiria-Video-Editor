//! The Edit page's viewer: the picture at the playhead, the transport over it,
//! and under it the playhead's and edit needle's timecodes and the preview quality.
//!
//! Pictures are made by the engine's preview worker. This only asks for the
//! one it needs (when the playhead, the project, the viewer's size or the
//! quality changes) and shows the newest that came back.
//!
//! A picture arrives as the decoder's YUV planes, which the GPU converts
//! while drawing (`ui_kit::video_picture`), or as RGBA for formats that can't
//! go that way and when the window has no GPU drawing to offer. Planes of a
//! picture decoded on the window's own graphics device are on the card
//! already and are drawn from there (`device_decoding`).
//!
//! When the window is drawn on a graphics card, the worker puts every
//! visible layer together there into one frame of the timeline's shape
//! (`engine::compose`), and this only shows that frame.
//!
//! The same viewer, made a second time ([`Viewer::second`]), is the Source
//! half of the split view: a worker of its own, asked for one file at a
//! moment ([`Viewer::source_ui`]), drawn from a picture slot of its own so
//! both pictures show at once. And what is on show can be handed to the
//! viewer's own window ([`Viewer::on_show`]), which draws it on the same
//! graphics device.
//!
//! Preview quality sets the size asked for. Planes come at the nearest
//! halving of the file's size that still fills it (less to copy and upload,
//! and a cleaner picture than the GPU shrinking a big one in one go); RGBA
//! pictures are scaled to it. On Auto, playback that can't keep up lowers
//! RGBA pictures further, since scaling them is real work on the CPU;
//! planes aren't lowered, because there is no such work left to save.
//! A picture decoded on the window's device always comes at the file's size:
//! nothing is copied to memory or uploaded, so there is nothing to save.

use crate::device_decoding::WindowDecoding;
use crate::video_picture;
use eframe::wgpu;
use engine::{Composing, Convert, Preview};
use media::{Decoded, Decoding};
use project::{Project, Rate, SequenceId, Time};
use std::sync::Arc;
use ui_kit::viewer::ZoomPick;
use ui_kit::{Action, PreviewQuality, Slot, Theme, TransportClick, VideoPicture};

/// The picture on show.
pub enum Shown {
    /// RGBA, converted on the CPU.
    Texture(egui::TextureHandle),
    /// YUV planes, converted by the GPU while drawing.
    Video(VideoPicture),
    /// The timeline's picture, put together by the worker. The frame is
    /// held for as long as it is on show, so the worker draws the next
    /// picture into another.
    Whole {
        picture: VideoPicture,
        _frame: engine::Composed,
    },
}

/// Counting a picture decoded on the window's device as drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Drawn {
    /// Nothing to count.
    No,
    /// On show, and not drawn yet.
    Waiting,
    /// Drawn in the last frame: counts once that frame is behind us.
    LastFrame,
}

/// What the viewer has on show, for drawing somewhere else (the viewer's
/// own window): the picture, held for as long as this is, and the
/// timeline's size. Cheap to clone, and can be handed to another window.
#[derive(Clone, Default)]
pub struct OnShow {
    pub shown: Option<Arc<Shown>>,
    pub timeline: (u32, u32),
}

impl OnShow {
    pub fn picture(&self) -> Option<ui_kit::viewer::Picture<'_>> {
        self.shown.as_deref().map(Shown::picture)
    }

    /// Whether this is the same picture as `other` (nothing new to draw).
    pub fn same(&self, other: &OnShow) -> bool {
        self.timeline == other.timeline
            && match (&self.shown, &other.shown) {
                (Some(a), Some(b)) => Arc::ptr_eq(a, b),
                (None, None) => true,
                _ => false,
            }
    }
}

impl Shown {
    fn picture(&self) -> ui_kit::viewer::Picture<'_> {
        match self {
            Shown::Texture(texture) => ui_kit::viewer::Picture::Texture(texture),
            Shown::Video(video) | Shown::Whole { picture: video, .. } => {
                ui_kit::viewer::Picture::Video(video)
            }
        }
    }
}

/// Draws the split view's Source in a place of the given size.
pub type DrawSource<'a> = dyn FnMut(&mut egui::Ui, egui::Vec2) + 'a;

/// Where the viewer's picture goes besides its own place.
#[derive(Default)]
pub struct Stage<'a> {
    /// Split view: draws the Source in the left half, given its size. The
    /// picture (the Program) takes the right half.
    pub source: Option<&'a mut DrawSource<'a>>,
    /// The picture is in the viewer's own window, which shows it this many
    /// pixels big (when known): a quiet note is in its place here, with the
    /// transport still under it.
    pub away: Option<Option<(u32, u32)>>,
}

/// Room between the two halves of a split viewer.
const SPLIT_GAP: f32 = 8.0;

pub struct Viewer {
    preview: Preview,
    /// Which picture slot it draws from: the viewer's, or the Source's.
    slot: Slot,
    ctx: egui::Context,
    convert: Convert,
    /// Decoding on the window's graphics device, while it is in use.
    decoding: Option<WindowDecoding>,
    /// The device and queue the window is drawn with, when the worker puts
    /// the timeline's picture together there.
    renderer: Option<(wgpu::Device, wgpu::Queue)>,
    drawn: Drawn,
    shown: Option<Arc<Shown>>,
    /// The size of the timeline whose picture is on show.
    timeline: (u32, u32),
    note: String,
    /// The last few pictures that took long, for Diagnostics.
    slow: crate::slow_pictures::SlowPictures,
    pub playhead: Time,
    /// What was last asked for, so the same request isn't sent every frame.
    requested: Option<(Project, Time, (u32, u32))>,
    /// The timeline time of the newest picture that came back since then.
    answered: Option<Time>,
    last_error: Option<String>,
    /// While playing on Auto: how far the picture is lowered to keep up (1, 1/2 or 1/4).
    play_scale: u32,
    /// Seconds per timeline frame while playing, to tell whether pictures keep up.
    frame_interval: Option<f64>,
    /// Where proxies are, handed to the worker (again when it is restarted).
    proxies: Option<Arc<cache::proxy::Shelf>>,
    /// A jump in a file without a proxy shows the key frame before it first
    /// (handed to the worker, again when it is restarted).
    key_frame_first: bool,
    /// How the picture is zoomed (Fit until changed; not kept).
    zoom: ui_kit::viewer::Zoom,
    /// Where the picture was last drawn (the viewer's room, the timeline's
    /// size and the pixels per point), for zoom commands.
    zoom_place: Option<(egui::Rect, (u32, u32), f32)>,
    /// A zoom asked for before the picture was first drawn: done then.
    zoom_waiting: Option<ZoomPick>,
    /// Playback loops inside the region: lights the transport's loop button.
    pub looping: bool,
}

impl Viewer {
    /// `start_at` places the playhead (seconds) before anything is imported.
    /// `convert` says whether the window can convert colour while drawing,
    /// `decoding` whether video is decoded on the window's graphics device,
    /// and `renderer` is the device and queue the window is drawn with (none
    /// when it isn't drawn on a graphics card, or colour is converted on
    /// the CPU: then the top clip's picture is shown alone).
    pub fn new(
        ctx: &egui::Context,
        start_at: Option<f64>,
        convert: Convert,
        decoding: Option<WindowDecoding>,
        renderer: Option<(wgpu::Device, wgpu::Queue)>,
    ) -> Viewer {
        let renderer = renderer.filter(|_| matches!(convert, Convert::WhileDrawing { .. }));
        Viewer {
            preview: start_preview(ctx, convert, decoding.as_ref(), renderer.as_ref()),
            slot: Slot::MAIN,
            ctx: ctx.clone(),
            convert,
            decoding,
            renderer,
            drawn: Drawn::No,
            shown: None,
            timeline: (16, 9),
            note: String::new(),
            slow: crate::slow_pictures::SlowPictures::default(),
            playhead: start_at
                .and_then(|s| Time::from_ratio((s * 1000.0) as i64, 1000))
                .unwrap_or(Time::ZERO),
            requested: None,
            answered: None,
            last_error: None,
            play_scale: 1,
            frame_interval: None,
            proxies: None,
            key_frame_first: true,
            zoom: ui_kit::viewer::Zoom::FIT,
            zoom_place: None,
            zoom_waiting: None,
            looping: false,
        }
    }

    /// Zooms as the zoom menu or a zoom command says: in or out a step
    /// about the middle, to fit, or to a scale.
    pub fn zoom_to(&mut self, pick: ZoomPick) {
        use ui_kit::viewer::Zoom;
        let Some((room, timeline, ppp)) = self.zoom_place else {
            self.zoom_waiting = Some(pick);
            return;
        };
        let middle = room.center();
        match pick {
            ZoomPick::Command(Action::ViewerZoomIn) => self.zoom.step(true, room, timeline, ppp),
            ZoomPick::Command(Action::ViewerZoomOut) => self.zoom.step(false, room, timeline, ppp),
            ZoomPick::Command(Action::ViewerZoom100) => {
                self.zoom.zoom_to_at(1.0, middle, room, timeline, ppp)
            }
            ZoomPick::Command(_) => self.zoom = Zoom::FIT,
            ZoomPick::Scale(scale) => self.zoom.zoom_to_at(scale, middle, room, timeline, ppp),
        }
        self.ctx.request_repaint();
    }

    /// The zoom in words, for the menu: "Fit · 47 %" or "200 %".
    fn zoom_readout(&self) -> String {
        match self.zoom_place {
            Some((room, timeline, ppp)) => self.zoom.readout(room, timeline, ppp, true),
            None => "Fit".to_owned(),
        }
    }

    /// Lets the viewer show frames from proxies (as the shelf's mode says).
    pub fn set_proxies(&mut self, shelf: Option<Arc<cache::proxy::Shelf>>) {
        self.preview.set_proxies(shelf.clone());
        self.proxies = shelf;
        self.requested = None;
    }

    /// Whether a jump in a file without a proxy shows the key frame before
    /// the moment first, then the exact frame (Settings › Proxies).
    pub fn set_key_frame_first(&mut self, on: bool) {
        if self.key_frame_first != on {
            self.key_frame_first = on;
            self.preview.set_key_frame_first(on);
        }
    }

    /// A second viewer beside this one, with a worker of its own on the
    /// same devices, drawing from `slot`: the split view's Source. Its
    /// worker keeps a quarter of the frames this one's may keep on the card.
    pub fn second(&self, slot: Slot) -> Viewer {
        let decoding = self.decoding.as_ref().map(|d| d.sharing(4));
        let mut viewer = Viewer::new(&self.ctx, None, self.convert, decoding, self.renderer.clone());
        viewer.slot = slot;
        viewer.set_proxies(self.proxies.clone());
        viewer.set_key_frame_first(self.key_frame_first);
        viewer
    }

    /// What is on show, for the viewer's own window to draw.
    pub fn on_show(&self) -> OnShow {
        OnShow {
            shown: self.shown.clone(),
            timeline: self.timeline,
        }
    }

    /// Playback started (`Some(seconds per frame)`) or stopped (`None`).
    pub fn set_playing(&mut self, frame_interval: Option<f64>) {
        self.frame_interval = frame_interval;
        self.preview.set_playing(frame_interval.is_some());
        if frame_interval.is_none() {
            self.play_scale = 1;
        }
    }

    /// Whether the picture last asked for has come back (true when none was
    /// asked for), so a screenshot shows what is at the playhead.
    pub fn settled(&self) -> bool {
        self.requested
            .as_ref()
            .is_none_or(|(_, at, _)| self.answered == Some(*at))
    }

    /// Takes in a finished picture. Returns an error to show, once per new problem.
    pub fn poll(&mut self, ctx: &egui::Context, quality: PreviewQuality) -> Option<diag::AppError> {
        self.count_drawn();
        let picture = self.preview.latest()?;
        // A first picture (a proxy frame, a key frame) is not the answer:
        // the exact one is still coming.
        if !picture.stand_in {
            self.answered = Some(picture.at);
        }
        if picture.frame.is_some() || picture.composed.is_some() {
            self.slow.note(
                picture.took,
                picture.method.as_deref().unwrap_or_default(),
                picture.work,
            );
        }
        // Drop resolution before dropping frames: on Auto, an RGBA picture
        // that took longer than a frame lowers the next ones. Planes cost
        // the same at any size, so lowering them would only blur: they
        // come at the size the viewer shows.
        match &picture.frame {
            Some(Decoded::Rgba(_)) => {
                if let Some(interval) = self.frame_interval
                    && quality == PreviewQuality::Auto
                    && picture.took.as_secs_f64() > interval
                    && self.play_scale < 4
                {
                    self.play_scale *= 2;
                }
            }
            Some(Decoded::Yuv(_) | Decoded::Gpu(_)) => self.play_scale = 1,
            None if picture.composed.is_some() => {
                if !picture.converted_on_cpu {
                    self.play_scale = 1;
                } else if let Some(interval) = self.frame_interval
                    && quality == PreviewQuality::Auto
                    && picture.took.as_secs_f64() > interval
                    && self.play_scale < 4
                {
                    self.play_scale *= 2;
                }
            }
            None => {}
        }
        let mut problem = None;
        if let Some(err) = picture.error {
            let text = err.report();
            if self.last_error.as_ref() != Some(&text) {
                self.last_error = Some(text);
                problem = Some(err);
            }
        } else {
            self.last_error = None;
        }
        // The renderer could not take a picture decoded on the window's
        // device when the worker put the timeline's picture together.
        if picture.refused {
            self.decode_the_usual_way();
            return problem;
        }
        let converted_on_cpu = picture.converted_on_cpu;
        let whole = picture.composed.map(|frame| {
            let (size, on_device) = (frame.size(), frame.on_device);
            let picture = VideoPicture::from_composed(ui_kit::ComposedPicture {
                frame: frame.view().clone(),
                working: frame.working(),
                size,
            })
            .in_slot(self.slot);
            let colour = if converted_on_cpu {
                "put together on the GPU · colour on the CPU"
            } else {
                "put together on the GPU"
            };
            (
                Shown::Whole {
                    picture,
                    _frame: frame,
                },
                colour,
                size,
                on_device,
            )
        });
        // A picture decoded on the window's device becomes the renderer's
        // textures here; one the renderer can't take is left out.
        let frame = match picture.frame {
            Some(Decoded::Gpu(frame)) => match self.decoding.as_ref().and_then(|d| d.picture(&frame)) {
                Some(video) => Some((
                    Shown::Video(video.in_slot(self.slot)),
                    "colour on the GPU",
                    frame.upright_size(),
                    true,
                )),
                None => {
                    self.decode_the_usual_way();
                    return problem;
                }
            },
            Some(Decoded::Yuv(planes)) => {
                let size = planes.upright_size();
                let video = VideoPicture::new(video_picture::image(planes)).in_slot(self.slot);
                Some((Shown::Video(video), "colour on the GPU", size, false))
            }
            Some(Decoded::Rgba(frame)) => {
                let size = (frame.width, frame.height);
                Some((
                    Shown::Texture(self.texture(ctx, &frame)),
                    "colour on the CPU",
                    size,
                    false,
                ))
            }
            None => whole,
        };
        self.drawn = Drawn::No;
        match frame {
            Some((shown, colour, (width, height), on_device)) => {
                let lowered = if self.play_scale > 1 {
                    format!(" · lowered to 1/{} to keep playing smoothly", self.play_scale)
                } else {
                    String::new()
                };
                // The corner says how the picture was decoded and which way
                // its colour was converted, for telling the ways apart.
                self.note = format!(
                    "{}{lowered} · {width}×{height} · {} ms · {} · {colour}",
                    quality.label(),
                    picture.took.as_millis(),
                    picture.method.unwrap_or_default()
                );
                self.shown = Some(Arc::new(shown));
                if on_device {
                    self.drawn = Drawn::Waiting;
                }
            }
            None => {
                self.shown = None;
                self.note = String::new();
            }
        }
        problem
    }

    /// What an export needs to decode and put pictures together the way
    /// this viewer does. `None` when the window isn't drawn on a graphics
    /// card (or converts colour on the CPU): there is nowhere to put the
    /// timeline's picture together then.
    pub fn export_devices(&self) -> Option<engine::export::Devices> {
        let (device, queue) = self.renderer.as_ref()?;
        Some(engine::export::Devices {
            decoding: Decoding::Best,
            convert: self.convert,
            on_device: self.decoding.as_ref().map(WindowDecoding::for_worker),
            composing: Composing {
                device: device.clone(),
                queue: queue.clone(),
                wrap: self.decoding.as_ref().map(WindowDecoding::wrap),
            },
        })
    }

    /// The pictures that took long lately, in words, if there are new ones
    /// since this was last asked: a line for Diagnostics.
    pub fn slow_pictures(&mut self) -> Option<String> {
        self.slow.take_changed()
    }

    /// The window is closing: the worker finishes the picture it is on and
    /// ends, so nothing is still decoding (on the window's graphics device
    /// or anywhere else) when the graphics go away. Waits a few seconds at most.
    pub fn close(&mut self) {
        self.frame_interval = None;
        if !self.preview.close(CLOSES_WITHIN) {
            tracing::warn!("the picture being decoded did not finish before the window closed");
        }
    }

    /// The renderer could not take a picture decoded on the window's device:
    /// from here on every file is decoded the way it was before, by a new
    /// worker without the device. The picture at the playhead is asked for again.
    fn decode_the_usual_way(&mut self) {
        tracing::warn!(
            "the renderer could not take a picture decoded on the window's device: video is decoded the usual way from now on"
        );
        if let Some(decoding) = self.decoding.take() {
            decoding.failed();
        }
        self.preview = start_preview(&self.ctx, self.convert, None, self.renderer.as_ref());
        self.preview.set_proxies(self.proxies.clone());
        self.preview.set_key_frame_first(self.key_frame_first);
        self.preview.set_playing(self.frame_interval.is_some());
        self.requested = None;
        self.ctx.request_repaint();
    }

    /// Tells the guard about a picture decoded on the window's device once
    /// the frame that drew it is behind us: only then did the driver get
    /// through sampling it.
    fn count_drawn(&mut self) {
        if self.drawn == Drawn::LastFrame {
            if let Some(decoding) = &self.decoding {
                decoding.drawn();
            }
            self.drawn = Drawn::No;
        }
    }

    /// An RGBA picture as a texture, written into the one on show when there is one.
    fn texture(&mut self, ctx: &egui::Context, frame: &media::Frame) -> egui::TextureHandle {
        let pixels = frame
            .rgba
            .chunks_exact(4)
            .map(|p| egui::Color32::from_rgb(p[0], p[1], p[2]))
            .collect();
        let image = egui::ColorImage::new([frame.width as usize, frame.height as usize], pixels);
        // The handle names one texture: a copy of it writes into the one on show.
        match self.shown.as_deref() {
            Some(Shown::Texture(texture)) => {
                let mut texture = texture.clone();
                texture.set(image, egui::TextureOptions::LINEAR);
                texture
            }
            _ => ctx.load_texture("viewer", image, egui::TextureOptions::LINEAR),
        }
    }

    /// Moves the playhead by whole frames, staying within the timeline.
    pub fn step(&mut self, frames: i64, rate: Rate, length: Time) {
        let frame = rate.frame_at(self.playhead) + frames;
        self.go_to(rate.frame_start(frame), rate, length);
    }

    /// Puts the playhead at `at`, on a frame, within the timeline.
    pub fn go_to(&mut self, at: Time, rate: Rate, length: Time) {
        let last = (length - rate.frame_duration()).max(Time::ZERO);
        self.playhead = rate.snap(at).max(Time::ZERO).min(last);
    }

    /// Draws the viewer, its transport and the timecodes in `height`, and asks
    /// for the picture it needs. Returns a command given on the viewer: a
    /// transport button, or a line of the Display menu (`display` says
    /// what is on). The tooltips and the menu name the keys in `shortcuts`.
    ///
    /// `stage` puts the Source beside the picture (split view), or says the
    /// picture is in its own window. `over_picture` draws what lies on the
    /// picture and under the transport (the handles on a selected clip's
    /// picture): it is given the frame's rectangle on screen and the
    /// viewer's own response, and since the transport comes after it, the
    /// transport's buttons keep their clicks. Not called while the picture
    /// is in its own window.
    #[allow(clippy::too_many_arguments)]
    pub fn ui(
        &mut self,
        ui: &mut egui::Ui,
        theme: &Theme,
        project: &Project,
        quality: &mut PreviewQuality,
        needle: Option<Time>,
        height: f32,
        shortcuts: &ui_kit::ShortcutTable,
        display: ui_kit::viewer::Display,
        stage: Stage<'_>,
        over_picture: impl FnOnce(&mut egui::Ui, egui::Rect, &egui::Response),
    ) -> Option<Action> {
        let seq = project.active()?;
        let rate = seq.settings.frame_rate;
        let timeline = (seq.settings.width, seq.settings.height);
        let size = egui::vec2(ui.available_width(), (height - 34.0).max(120.0));
        let Stage { source, away } = stage;
        let split = source.is_some();
        let half = egui::vec2(((size.x - SPLIT_GAP) / 2.0).max(60.0), size.y);
        let own = if split { half } else { size };
        let mut command = None;
        let ((frame, seen), playing) = (
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = SPLIT_GAP;
                if let Some(source) = source {
                    source(ui, half);
                }
                if away.is_some() {
                    let (area, _) = ui_kit::viewer::empty(ui, theme, own, "The viewer is in its own window");
                    let button = egui::Rect::from_center_size(
                        area.center() + egui::vec2(0.0, 30.0),
                        egui::vec2(140.0, 26.0),
                    );
                    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(button));
                    if child
                        .add_sized(button.size(), egui::Button::new("Bring it back"))
                        .on_hover_text(shortcuts.tip(Action::ViewerWindow))
                        .clicked()
                    {
                        command = Some(Action::ViewerWindow);
                    }
                    (area.shrink(8.0), area.shrink(8.0))
                } else {
                    // With nothing over the picture, not even its note.
                    let note = if display.clean {
                        String::new()
                    } else {
                        self.note.clone()
                    };
                    let (frame, response) = self.zoomed_picture_ui(ui, theme, own, timeline, &note);
                    // The handles line up with the whole frame, zoomed or not.
                    if !display.clean {
                        over_picture(ui, frame, &response);
                    }
                    let seen = ui_kit::viewer::seen(frame, response.rect);
                    // Over the handles, so it can always be read.
                    if split && !display.clean {
                        ui_kit::viewer::caption(ui, theme, seen, "Program");
                    }
                    (frame, seen)
                }
            })
            .inner,
            self.frame_interval.is_some(),
        );
        if !display.clean {
            let click = ui_kit::transport::transport(ui, theme, seen, playing, Some(self.looping), shortcuts);
            command = command.or(click.map(TransportClick::action));
        }

        ui.add_space(6.0);
        ui.horizontal(|ui| {
            let time = ui
                .label(egui::RichText::new(self.playhead.timecode(rate)).font(ui_kit::fonts::mono(13.0)))
                .on_hover_text("Playhead");
            // With nothing over the picture, the play buttons are here,
            // under its middle.
            if display.clean {
                let centre = egui::pos2(seen.center().x, time.rect.center().y);
                let looping = Some(self.looping);
                let click = ui_kit::transport::compact(ui, theme, centre, playing, looping, shortcuts);
                command = command.or(click.map(TransportClick::action));
            }
            if let Some(needle) = needle {
                ui.label(
                    egui::RichText::new(needle.timecode(rate))
                        .font(ui_kit::fonts::mono(13.0))
                        .color(ui_kit::status::EDIT_NEEDLE),
                )
                .on_hover_text("Edit needle: cuts, splits and labels happen here");
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                egui::ComboBox::from_id_salt("preview-quality")
                    .width(100.0)
                    .selected_text(format!("Preview {}", quality.label()))
                    .show_ui(ui, |ui| {
                        for q in PreviewQuality::ALL {
                            ui.selectable_value(quality, q, q.label());
                        }
                    });
                if away.is_none() {
                    let readout = self.zoom_readout();
                    if let Some(pick) = ui_kit::viewer::zoom_menu(ui, &self.zoom, &readout, shortcuts) {
                        match pick {
                            ZoomPick::Command(action) => command = Some(action),
                            ZoomPick::Scale(_) => self.zoom_to(pick),
                        }
                    }
                }
                if let Some(picked) = ui_kit::viewer::display_menu(ui, display, shortcuts) {
                    command = Some(picked);
                }
            });
        });

        // Ask for the picture this needs, sized for where it is shown.
        let ppp = ui.ctx().pixels_per_point();
        let here = ((frame.width() * ppp) as u32, (frame.height() * ppp) as u32);
        let on_screen = away.flatten().unwrap_or(here);
        self.ask(project, seq.id, self.playhead, timeline, on_screen, *quality);
        command
    }

    /// Draws the picture on show in `size`, in a frame of `timeline`'s
    /// shape, with `note` in the corner. Counts a picture decoded on the
    /// window's device as drawn.
    fn picture_ui(
        &mut self,
        ui: &mut egui::Ui,
        theme: &Theme,
        size: egui::Vec2,
        timeline: (u32, u32),
        note: &str,
    ) -> (egui::Rect, egui::Response) {
        self.count_drawn();
        self.mark_drawn(ui.ctx());
        let picture = self.shown.as_deref().map(Shown::picture);
        ui_kit::viewer::viewer(ui, theme, size, timeline, picture, note)
    }

    /// [`Viewer::picture_ui`] zoomed as the viewer's zoom says, reading the
    /// hand's zooming and panning: the Program's picture.
    fn zoomed_picture_ui(
        &mut self,
        ui: &mut egui::Ui,
        theme: &Theme,
        size: egui::Vec2,
        timeline: (u32, u32),
        note: &str,
    ) -> (egui::Rect, egui::Response) {
        self.count_drawn();
        self.mark_drawn(ui.ctx());
        let picture = self.shown.as_deref().map(Shown::picture);
        let mut zoom = self.zoom;
        let drawn = ui_kit::viewer::zoomed_viewer(ui, theme, size, timeline, picture, note, &mut zoom);
        self.zoom = zoom;
        self.zoom_place = Some((
            ui_kit::viewer::room(drawn.1.rect),
            timeline,
            ui.ctx().pixels_per_point(),
        ));
        if let Some(pick) = self.zoom_waiting.take() {
            self.zoom_to(pick);
        }
        drawn
    }

    /// The picture on show is drawn this frame (here, or in the viewer's
    /// own window): a picture decoded on the window's device counts in the next.
    pub fn mark_drawn(&mut self, ctx: &egui::Context) {
        if self.drawn == Drawn::Waiting {
            self.drawn = Drawn::LastFrame;
            ctx.request_repaint();
        }
    }

    /// The split view's Source: the file of `alone` (a project of one file,
    /// `engine::source::file_alone`) at `at`, a time in the file, drawn in
    /// `size` in the file's own shape with `caption` over it.
    #[allow(clippy::too_many_arguments)]
    pub fn source_ui(
        &mut self,
        ui: &mut egui::Ui,
        theme: &Theme,
        size: egui::Vec2,
        alone: &Project,
        at: Time,
        quality: PreviewQuality,
        caption: &str,
    ) {
        let Some(seq) = alone.active() else { return };
        let shape = (seq.settings.width, seq.settings.height);
        let (frame, _) = self.picture_ui(ui, theme, size, shape, "");
        ui_kit::viewer::caption(ui, theme, frame, caption);
        let ppp = ui.ctx().pixels_per_point();
        let on_screen = ((frame.width() * ppp) as u32, (frame.height() * ppp) as u32);
        self.ask(alone, seq.id, at, shape, on_screen, quality);
    }

    /// Asks for the picture at `at` on `sequence` of `project`, for a frame
    /// `on_screen` pixels big, unless that is what was asked for last.
    fn ask(
        &mut self,
        project: &Project,
        sequence: SequenceId,
        at: Time,
        timeline: (u32, u32),
        on_screen: (u32, u32),
        quality: PreviewQuality,
    ) {
        self.timeline = timeline;
        let (w, h) = quality.picture_size(timeline, on_screen);
        let want = ((w / self.play_scale).max(2), (h / self.play_scale).max(2));
        let same = self
            .requested
            .as_ref()
            .is_some_and(|(p, asked, size)| *asked == at && *size == want && p == project);
        if !same {
            self.preview.show(project, sequence, at, want.0, want.1);
            self.requested = Some((project.clone(), at, want));
            self.answered = None;
        }
    }
}

/// How long closing the app waits for the picture being decoded to finish.
const CLOSES_WITHIN: std::time::Duration = std::time::Duration::from_secs(3);

/// Starts the preview worker, on the window's graphics device when there is one.
/// With `renderer` (what the window is drawn with) the worker also puts the
/// timeline's whole picture together there.
fn start_preview(
    ctx: &egui::Context,
    convert: Convert,
    decoding: Option<&WindowDecoding>,
    renderer: Option<&(wgpu::Device, wgpu::Queue)>,
) -> Preview {
    let ctx = ctx.clone();
    let composing = renderer.map(|(device, queue)| Composing {
        device: device.clone(),
        queue: queue.clone(),
        wrap: decoding.map(WindowDecoding::wrap),
    });
    Preview::start_composing(
        Decoding::Best,
        convert,
        decoding.map(WindowDecoding::for_worker),
        composing,
        move || ctx.request_repaint(),
    )
}
