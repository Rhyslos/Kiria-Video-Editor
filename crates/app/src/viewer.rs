//! The Edit page's viewer: the picture at the playhead, a scrub bar under it
//! (until the timeline arrives) and the preview quality.
//!
//! Pictures are made by the engine's preview worker. This only asks for the
//! one it needs (when the playhead, the project, the viewer's size or the
//! quality changes) and shows the newest that came back.

use engine::Preview;
use media::Decoding;
use project::{Project, Rate, Time};
use ui_kit::{PreviewQuality, Theme, TransportClick};

pub struct Viewer {
    preview: Preview,
    texture: Option<egui::TextureHandle>,
    note: String,
    pub playhead: Time,
    /// What was last asked for, so the same request isn't sent every frame.
    requested: Option<(Project, Time, (u32, u32))>,
    last_error: Option<String>,
    /// While playing on Auto: how far the picture is lowered to keep up (1, 1/2 or 1/4).
    play_scale: u32,
    /// Seconds per timeline frame while playing, to tell whether pictures keep up.
    frame_interval: Option<f64>,
}

impl Viewer {
    /// `start_at` places the playhead (seconds) before anything is imported.
    pub fn new(ctx: &egui::Context, start_at: Option<f64>) -> Viewer {
        let ctx = ctx.clone();
        Viewer {
            preview: Preview::start(Decoding::Best, move || ctx.request_repaint()),
            texture: None,
            note: String::new(),
            playhead: start_at
                .and_then(|s| Time::from_ratio((s * 1000.0) as i64, 1000))
                .unwrap_or(Time::ZERO),
            requested: None,
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

    /// Takes in a finished picture. Returns an error to show, once per new problem.
    pub fn poll(&mut self, ctx: &egui::Context, quality: PreviewQuality) -> Option<diag::AppError> {
        let picture = self.preview.latest()?;
        // Drop resolution before dropping frames: on Auto, a picture that
        // took longer than a frame lowers the next ones.
        if let Some(interval) = self.frame_interval
            && quality == PreviewQuality::Auto
            && picture.took.as_secs_f64() > interval
            && self.play_scale < 4
        {
            self.play_scale *= 2;
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
                let pixels = frame
                    .rgba
                    .chunks_exact(4)
                    .map(|p| egui::Color32::from_rgb(p[0], p[1], p[2]))
                    .collect();
                let image = egui::ColorImage::new([frame.width as usize, frame.height as usize], pixels);
                match &mut self.texture {
                    Some(t) => t.set(image, egui::TextureOptions::LINEAR),
                    None => {
                        self.texture = Some(ctx.load_texture("viewer", image, egui::TextureOptions::LINEAR))
                    }
                }
                let lowered = if self.play_scale > 1 {
                    format!(" · lowered to 1/{} to keep playing smoothly", self.play_scale)
                } else {
                    String::new()
                };
                self.note = format!(
                    "{}{lowered} · {}×{} · {} ms · {}",
                    quality.label(),
                    frame.width,
                    frame.height,
                    picture.took.as_millis(),
                    picture.method.unwrap_or_default()
                );
            }
            None => {
                self.texture = None;
                self.note = String::new();
            }
        }
        problem
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

    /// Draws the viewer, its transport and the scrub bar in `height`, and asks
    /// for the picture it needs. Returns a transport button pressed.
    pub fn ui(
        &mut self,
        ui: &mut egui::Ui,
        theme: &Theme,
        project: &Project,
        quality: &mut PreviewQuality,
        height: f32,
    ) -> Option<TransportClick> {
        let seq = project.active()?;
        let (rate, length) = (seq.settings.frame_rate, seq.length());
        let timeline = (seq.settings.width, seq.settings.height);
        let size = egui::vec2(ui.available_width(), (height - 34.0).max(120.0));
        let (frame, _) = ui_kit::viewer::viewer(ui, theme, size, timeline, self.texture.as_ref(), &self.note);
        let click = ui_kit::transport::transport(ui, theme, frame, self.frame_interval.is_some());

        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(self.playhead.timecode(rate)).font(ui_kit::fonts::mono(13.0)));
            let quality_width = 110.0;
            let mut seconds = self.playhead.as_secs_f64();
            let end = length.as_secs_f64().max(0.001);
            ui.spacing_mut().slider_width = (ui.available_width() - quality_width - 16.0).max(80.0);
            let slider = ui.add_enabled(
                length > Time::ZERO,
                egui::Slider::new(&mut seconds, 0.0..=end).show_value(false),
            );
            if slider.changed() {
                let at = Time::from_ratio((seconds * 1_000_000.0) as i64, 1_000_000).unwrap_or(Time::ZERO);
                self.go_to(at, rate, length);
            }
            egui::ComboBox::from_id_salt("preview-quality")
                .width(quality_width - 10.0)
                .selected_text(format!("Preview {}", quality.label()))
                .show_ui(ui, |ui| {
                    for q in PreviewQuality::ALL {
                        ui.selectable_value(quality, q, q.label());
                    }
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
        }
        click
    }
}
