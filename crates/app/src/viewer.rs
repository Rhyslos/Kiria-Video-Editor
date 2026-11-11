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
use project::{Project, Rate, Time};
use ui_kit::{PreviewQuality, Theme, TransportClick, VideoPicture};

/// The picture on show.
enum Shown {
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

pub struct Viewer {
    preview: Preview,
    ctx: egui::Context,
    convert: Convert,
    /// Decoding on the window's graphics device, while it is in use.
    decoding: Option<WindowDecoding>,
    /// The device and queue the window is drawn with, when the worker puts
    /// the timeline's picture together there.
    renderer: Option<(wgpu::Device, wgpu::Queue)>,
    drawn: Drawn,
    shown: Option<Shown>,
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
            ctx: ctx.clone(),
            convert,
            decoding,
            renderer,
            drawn: Drawn::No,
            shown: None,
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
        }
    }

    /// Playback started (`Some(seconds per frame)`) or stopped (`None`).
    pub fn set_playing(&mut self, frame_interval: Option<f64>) {
        self.frame_interval = frame_interval;
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
        self.answered = Some(picture.at);
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
            });
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
                    Shown::Video(video),
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
                let video = VideoPicture::new(video_picture::image(planes));
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
                self.shown = Some(shown);
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
        match self.shown.take() {
            Some(Shown::Texture(mut texture)) => {
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
    /// for the picture it needs. Returns a transport button pressed. The
    /// transport's tooltips name the keys in `shortcuts`.
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
    ) -> Option<TransportClick> {
        let seq = project.active()?;
        let rate = seq.settings.frame_rate;
        let timeline = (seq.settings.width, seq.settings.height);
        let size = egui::vec2(ui.available_width(), (height - 34.0).max(120.0));
        self.count_drawn();
        if self.drawn == Drawn::Waiting {
            // Drawn below, in this frame; counted in the next.
            self.drawn = Drawn::LastFrame;
            ui.ctx().request_repaint();
        }
        let picture = self.shown.as_ref().map(|shown| match shown {
            Shown::Texture(texture) => ui_kit::viewer::Picture::Texture(texture),
            Shown::Video(video) | Shown::Whole { picture: video, .. } => {
                ui_kit::viewer::Picture::Video(video)
            }
        });
        let (frame, _) = ui_kit::viewer::viewer(ui, theme, size, timeline, picture, &self.note);
        let click = ui_kit::transport::transport(ui, theme, frame, self.frame_interval.is_some(), shortcuts);

        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(self.playhead.timecode(rate)).font(ui_kit::fonts::mono(13.0)))
                .on_hover_text("Playhead");
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
            });
        });

        // Ask for the picture this needs, if it isn't the one already asked for.
        let ppp = ui.ctx().pixels_per_point();
        let on_screen = ((frame.width() * ppp) as u32, (frame.height() * ppp) as u32);
        let (w, h) = quality.picture_size(timeline, on_screen);
        let want = ((w / self.play_scale).max(2), (h / self.play_scale).max(2));
        let same = self
            .requested
            .as_ref()
            .is_some_and(|(p, at, size)| *at == self.playhead && *size == want && p == project);
        if !same {
            self.preview.show(project, seq.id, self.playhead, want.0, want.1);
            self.requested = Some((project.clone(), self.playhead, want));
            self.answered = None;
        }
        click
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
