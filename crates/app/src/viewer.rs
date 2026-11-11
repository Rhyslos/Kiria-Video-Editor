//! The Edit page's viewer: the picture at the playhead, the transport over it,
//! and under it the playhead's and edit needle's timecodes and the preview quality.
//!
//! Pictures are made by the engine's preview worker. This only asks for the
//! one it needs (when the playhead, the project, the viewer's size or the
//! quality changes) and shows the newest that came back.
//!
//! A picture arrives as the decoder's YUV planes, which the GPU converts
//! while drawing (`ui_kit::video_picture`), or as RGBA for formats that can't
//! go that way and when the window has no GPU drawing to offer.
//!
//! Preview quality sets the size asked for. Planes come at the nearest
//! halving of the file's size that still fills it (less to copy and upload,
//! and a cleaner picture than the GPU shrinking a big one in one go); RGBA
//! pictures are scaled to it. On Auto, playback that can't keep up lowers
//! RGBA pictures further, since scaling them is real work on the CPU;
//! planes aren't lowered, because there is no such work left to save.

use crate::video_picture;
use engine::{Convert, Preview};
use media::{Decoded, Decoding};
use project::{Project, Rate, Time};
use ui_kit::{PreviewQuality, Theme, TransportClick, VideoPicture};

/// The picture on show.
enum Shown {
    /// RGBA, converted on the CPU.
    Texture(egui::TextureHandle),
    /// YUV planes, converted by the GPU while drawing.
    Video(VideoPicture),
}

pub struct Viewer {
    preview: Preview,
    shown: Option<Shown>,
    note: String,
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
    /// `convert` says whether the window can convert colour while drawing.
    pub fn new(ctx: &egui::Context, start_at: Option<f64>, convert: Convert) -> Viewer {
        let ctx = ctx.clone();
        Viewer {
            preview: Preview::start(Decoding::Best, convert, move || ctx.request_repaint()),
            shown: None,
            note: String::new(),
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
        let picture = self.preview.latest()?;
        self.answered = Some(picture.at);
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
            Some(Decoded::Yuv(_)) => self.play_scale = 1,
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
        match picture.frame {
            Some(frame) => {
                let (width, height) = frame.size();
                let lowered = if self.play_scale > 1 {
                    format!(" · lowered to 1/{} to keep playing smoothly", self.play_scale)
                } else {
                    String::new()
                };
                // Which way the colour was converted, for telling the two apart.
                let colour = match &frame {
                    Decoded::Yuv(_) => "colour on the GPU",
                    Decoded::Rgba(_) => "colour on the CPU",
                };
                self.note = format!(
                    "{}{lowered} · {width}×{height} · {} ms · {} · {colour}",
                    quality.label(),
                    picture.took.as_millis(),
                    picture.method.unwrap_or_default()
                );
                self.shown = Some(match frame {
                    Decoded::Yuv(planes) => Shown::Video(VideoPicture::new(video_picture::image(planes))),
                    Decoded::Rgba(frame) => Shown::Texture(self.texture(ctx, &frame)),
                });
            }
            None => {
                self.shown = None;
                self.note = String::new();
            }
        }
        problem
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
    /// for the picture it needs. Returns a transport button pressed.
    pub fn ui(
        &mut self,
        ui: &mut egui::Ui,
        theme: &Theme,
        project: &Project,
        quality: &mut PreviewQuality,
        needle: Option<Time>,
        height: f32,
    ) -> Option<TransportClick> {
        let seq = project.active()?;
        let rate = seq.settings.frame_rate;
        let timeline = (seq.settings.width, seq.settings.height);
        let size = egui::vec2(ui.available_width(), (height - 34.0).max(120.0));
        let picture = self.shown.as_ref().map(|shown| match shown {
            Shown::Texture(texture) => ui_kit::viewer::Picture::Texture(texture),
            Shown::Video(video) => ui_kit::viewer::Picture::Video(video),
        });
        let (frame, _) = ui_kit::viewer::viewer(ui, theme, size, timeline, picture, &self.note);
        let click = ui_kit::transport::transport(ui, theme, frame, self.frame_interval.is_some());

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
