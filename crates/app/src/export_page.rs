//! The Export page, in its first form: the timeline (or its region) written
//! as one MP4, with a file to choose, a quality, and progress with Cancel.
//! Presets, the queue and other formats come with the full page.
//!
//! The work is the engine's (`engine::export`), on a worker; editing goes on
//! meanwhile, and what is exported is the timeline as it was when Export
//! was pressed.

use diag::AppError;
use engine::export::{Devices, Export, ExportSettings, Exported, Outcome, Progress, Quality};
use project::{Project, Time};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, channel};
use std::time::Duration;
use ui_kit::Theme;

/// How the last export ended, to say so until the next one starts.
#[derive(Debug)]
enum Ended {
    Done(Exported),
    Cancelled,
}

#[derive(Debug, Default)]
pub struct ExportPage {
    /// The file chosen; until one is, the project's name beside the project.
    chosen: Option<PathBuf>,
    /// Export the region only (when there is one).
    region_only: bool,
    quality: Quality,
    software: bool,
    /// The save dialog, while it is open.
    dialog: Option<Receiver<Option<PathBuf>>>,
    running: Option<Export>,
    ended: Option<Ended>,
}

impl ExportPage {
    /// Stops the export under way, if any, and waits briefly for it to let
    /// go of its file: for closing the app.
    pub fn stop(&mut self) {
        if let Some(export) = self.running.take() {
            export.cancel();
            let give_up = std::time::Instant::now() + Duration::from_secs(5);
            while export.finished().is_none() && std::time::Instant::now() < give_up {
                std::thread::sleep(Duration::from_millis(20));
            }
        }
    }

    /// Takes in what happened since the last frame. Returns an error to show.
    pub fn poll(&mut self) -> Option<AppError> {
        if let Some(dialog) = &self.dialog
            && let Ok(picked) = dialog.try_recv()
        {
            self.dialog = None;
            if let Some(path) = picked {
                self.chosen = Some(with_mp4(path));
            }
        }
        let outcome = self.running.as_ref()?.finished()?;
        self.running = None;
        match outcome {
            Outcome::Done(done) => {
                tracing::info!(
                    "exported {} frames to {} in {:.1} s with {}",
                    done.frames,
                    done.path.display(),
                    done.took.as_secs_f64(),
                    done.encoder
                );
                self.ended = Some(Ended::Done(done));
                None
            }
            Outcome::Cancelled => {
                self.ended = Some(Ended::Cancelled);
                None
            }
            Outcome::Failed(err) => {
                self.ended = None;
                Some(err)
            }
        }
    }

    /// Draws the page. `devices` gives what the worker needs when Export is
    /// pressed; `None` from it means this window can't put pictures together.
    pub fn ui(
        &mut self,
        ui: &mut egui::Ui,
        theme: &Theme,
        project: &Project,
        project_file: &Path,
        devices: impl FnOnce() -> Option<Devices>,
    ) -> Option<AppError> {
        let mut problem = None;
        let seq = project.active()?;
        let rate = seq.settings.frame_rate;
        let length = seq.length();
        let region = seq
            .region
            .map(|r| (r.start, r.end.min(length)))
            .filter(|(from, to)| to > from);
        if region.is_none() {
            self.region_only = false;
        }
        let range = if self.region_only { region } else { None };
        let stretch = range.map_or(length, |(from, to)| to - from);
        let size = (seq.settings.width & !1, seq.settings.height & !1);
        let path = self
            .chosen
            .clone()
            .unwrap_or_else(|| default_path(project_file, &project.name));

        ui.vertical_centered(|ui| {
            ui.set_max_width(560.0);
            ui.add_space(24.0);
            ui.label(
                egui::RichText::new("Export")
                    .family(ui_kit::fonts::semibold())
                    .size(22.0),
            );
            ui.label(
                egui::RichText::new(format!(
                    "{}×{} · {} frames a second · H.264 with AAC sound, in an MP4",
                    size.0,
                    size.1,
                    short(rate.as_f64())
                ))
                .color(theme.muted),
            );
            ui.add_space(18.0);
        });
        let width = 560.0_f32.min(ui.available_width());
        let side = ((ui.available_width() - width) / 2.0).max(0.0);
        ui.horizontal_top(|ui| {
            ui.add_space(side);
            ui.vertical(|ui| {
                ui.set_width(width);
                self.form(
                    ui,
                    theme,
                    project,
                    &path,
                    (region, range, stretch),
                    devices,
                    &mut problem,
                );
            });
        });
        problem
    }

    /// The form, the Export button and how it is going, in the page's column.
    #[allow(clippy::too_many_arguments)]
    fn form(
        &mut self,
        ui: &mut egui::Ui,
        theme: &Theme,
        project: &Project,
        path: &Path,
        (region, range, stretch): (Stretch, Stretch, Time),
        devices: impl FnOnce() -> Option<Devices>,
        problem: &mut Option<AppError>,
    ) {
        let Some(seq) = project.active() else { return };
        let rate = seq.settings.frame_rate;
        let length = seq.length();
        let size = (seq.settings.width & !1, seq.settings.height & !1);
        let running = self.running.is_some();
        ui.add_enabled_ui(!running, |ui| {
            row(ui, theme, "File", |ui| {
                if ui
                    .button("Choose…")
                    .on_hover_text("Pick where the video is saved and what it is called")
                    .clicked()
                    && self.dialog.is_none()
                {
                    self.dialog = Some(ask_where(ui.ctx(), path));
                }
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(path.display().to_string()).color(theme.text_secondary),
                    )
                    .truncate(),
                )
                .on_hover_text(path.display().to_string());
            });
            row(ui, theme, "What", |ui| {
                ui.vertical(|ui| {
                    ui.radio_value(
                        &mut self.region_only,
                        false,
                        format!("The whole timeline ({})", length.timecode(rate)),
                    );
                    let label = match region {
                        Some((from, to)) => {
                            format!("The region ({} to {})", from.timecode(rate), to.timecode(rate))
                        }
                        None => "The region (none is marked: Shift-drag in the timeline's ruler)".to_owned(),
                    };
                    ui.add_enabled(region.is_some(), egui::RadioButton::new(self.region_only, label))
                        .clicked()
                        .then(|| self.region_only = true);
                });
            });
            row(ui, theme, "Quality", |ui| {
                ui.vertical(|ui| {
                    for quality in Quality::ALL {
                        let megabytes = megabytes(quality.bit_rate(size, rate), stretch);
                        let about = match quality {
                            Quality::High => "for keeping, or for a site that encodes it again",
                            Quality::Medium => "a good picture at a sensible size",
                            Quality::Small => "for sending",
                        };
                        ui.radio_value(
                            &mut self.quality,
                            quality,
                            format!(
                                "{} · about {} · {about}",
                                quality.label(),
                                megabytes_text(megabytes)
                            ),
                        );
                    }
                });
            });
            row(ui, theme, "Encoder", |ui| {
                ui.vertical(|ui| {
                    ui.radio_value(
                        &mut self.software,
                        false,
                        "Automatic: the graphics card when it can, else the processor",
                    );
                    ui.radio_value(
                        &mut self.software,
                        true,
                        "Processor only: slower, and works everywhere",
                    );
                });
            });
        });
        ui.add_space(14.0);

        match &self.running {
            Some(export) => {
                let progress = export.progress();
                ui.add(egui::ProgressBar::new(progress.share()).desired_height(10.0));
                ui.add_space(4.0);
                ui.label(
                    egui::RichText::new(progress_text(&progress, rate.as_f64())).color(theme.text_secondary),
                );
                ui.add_space(8.0);
                if ui
                    .button("Cancel")
                    .on_hover_text("Stop; nothing is left behind")
                    .clicked()
                {
                    export.cancel();
                }
                // Keep the numbers moving.
                ui.ctx().request_repaint_after(Duration::from_millis(100));
            }
            None => {
                let empty = stretch <= Time::ZERO;
                let devices = if empty { None } else { devices() };
                let why_not = if empty {
                    Some("There is nothing on the timeline to export yet.")
                } else if devices.is_none() {
                    Some(
                        "This window isn't drawn on a graphics card, so pictures can't be put together for a file here.",
                    )
                } else {
                    None
                };
                ui.horizontal(|ui| {
                    let button = ui.add_enabled(why_not.is_none(), |ui: &mut egui::Ui| {
                        ui_kit::accent_button(ui, theme, "Export")
                    });
                    if button.clicked()
                        && let Some(devices) = devices
                    {
                        match prepare(path) {
                            Ok(()) => {
                                let settings = ExportSettings {
                                    path: path.to_path_buf(),
                                    range,
                                    quality: self.quality,
                                    software: self.software,
                                };
                                let ctx = ui.ctx().clone();
                                self.ended = None;
                                self.running = Some(Export::start(
                                    project.clone(),
                                    project.active_sequence,
                                    settings,
                                    devices,
                                    move || ctx.request_repaint(),
                                ));
                            }
                            Err(err) => *problem = Some(err),
                        }
                    }
                    if let Some(why) = why_not {
                        ui.label(egui::RichText::new(why).color(theme.muted));
                    }
                });
                ui.add_space(10.0);
                match &self.ended {
                    Some(Ended::Done(done)) => {
                        ui.label(egui::RichText::new(done_text(done)).color(ui_kit::status::GOOD));
                        ui.label(
                            egui::RichText::new(done.path.display().to_string()).color(theme.text_secondary),
                        );
                        if ui.button("Show in folder").clicked()
                            && let Err(err) = show_in_folder(&done.path)
                        {
                            *problem = Some(err);
                        }
                    }
                    Some(Ended::Cancelled) => {
                        ui.label(egui::RichText::new("Cancelled. Nothing was saved.").color(theme.muted));
                    }
                    None => {}
                }
            }
        }
    }
}

/// A stretch of the timeline, from and to; none for all of it (or no region).
type Stretch = Option<(Time, Time)>;

/// A row: a name on the left, what it is about on the right.
fn row(ui: &mut egui::Ui, theme: &Theme, label: &str, values: impl FnOnce(&mut egui::Ui)) {
    ui.horizontal_top(|ui| {
        ui_kit::inspector::label_cell(ui, theme, label, 84.0);
        values(ui);
    });
    ui.add_space(8.0);
}

/// Opens the save dialog on its own thread; the choice arrives later.
fn ask_where(ctx: &egui::Context, now: &Path) -> Receiver<Option<PathBuf>> {
    let (tell, chosen) = channel();
    let ctx = ctx.clone();
    let folder = now.parent().map(Path::to_path_buf);
    let name = now.file_name().map(|n| n.to_string_lossy().into_owned());
    std::thread::spawn(move || {
        let mut dialog = rfd::FileDialog::new()
            .set_title("Export the video as")
            .add_filter("MP4 video", &["mp4"]);
        if let Some(folder) = folder {
            dialog = dialog.set_directory(folder);
        }
        if let Some(name) = name {
            dialog = dialog.set_file_name(name);
        }
        let _ = tell.send(dialog.save_file());
        ctx.request_repaint();
    });
    chosen
}

/// Checks the file can be written where it is to go, before the work starts.
fn prepare(path: &Path) -> diag::Result<()> {
    let folder = path.parent().unwrap_or(Path::new("."));
    if !folder.is_dir() {
        return Err(AppError::new("The video can't be exported there")
            .why(format!("The folder {} does not exist.", folder.display()))
            .fix("Choose another place with Choose…."));
    }
    Ok(())
}

fn show_in_folder(path: &Path) -> diag::Result<()> {
    let folder = path.parent().unwrap_or(path);
    let program = if cfg!(windows) { "explorer" } else { "xdg-open" };
    std::process::Command::new(program)
        .arg(folder)
        .spawn()
        .map_err(|err| {
            AppError::new("The folder can't be shown")
                .why(format!(
                    "Starting the file manager for {} failed.",
                    folder.display()
                ))
                .fix("Open the folder yourself; its path is shown above.")
                .source(err)
        })?;
    Ok(())
}

/// "<project name>.mp4" beside the project file.
fn default_path(project_file: &Path, name: &str) -> PathBuf {
    let safe: String = name
        .chars()
        .map(|c| {
            if r#"\/:*?"<>|"#.contains(c) || c.is_control() {
                '-'
            } else {
                c
            }
        })
        .collect();
    let safe = safe.trim().trim_matches('.');
    let file = format!("{}.mp4", if safe.is_empty() { "Video" } else { safe });
    project_file.parent().unwrap_or(Path::new(".")).join(file)
}

/// The chosen file name, ending in `.mp4`.
fn with_mp4(path: PathBuf) -> PathBuf {
    if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("mp4")) {
        path
    } else {
        let mut name = path.into_os_string();
        name.push(".mp4");
        PathBuf::from(name)
    }
}

/// The size of `length` of video at `bits` a second, with its sound, in megabytes.
fn megabytes(bits: u64, length: Time) -> f64 {
    (bits + 192_000) as f64 * length.as_secs_f64().max(0.0) / 8.0 / 1_000_000.0
}

fn megabytes_text(megabytes: f64) -> String {
    if megabytes >= 1000.0 {
        format!("{:.1} GB", megabytes / 1000.0)
    } else if megabytes >= 10.0 {
        format!("{megabytes:.0} MB")
    } else {
        format!("{megabytes:.1} MB")
    }
}

/// A number without a needless ".00" ("60", "59.94").
fn short(value: f64) -> String {
    let text = format!("{value:.2}");
    text.trim_end_matches('0').trim_end_matches('.').to_owned()
}

fn seconds_text(time: Duration) -> String {
    let seconds = time.as_secs();
    if seconds >= 60 {
        format!("{} min {} s", seconds / 60, seconds % 60)
    } else {
        format!("{seconds} s")
    }
}

/// How an encoder is named for people.
fn encoder_text(name: &str) -> String {
    if name == "libx264" {
        "encoded on the processor".to_owned()
    } else {
        format!("encoded on the graphics card ({name})")
    }
}

fn progress_text(progress: &Progress, frames_a_second: f64) -> String {
    let mut parts = vec![format!("Frame {} of {}", progress.frames_done, progress.frames)];
    let elapsed = progress.elapsed.as_secs_f64();
    if progress.frames_done >= 10 && elapsed > 0.0 {
        let pace = progress.frames_done as f64 / elapsed / frames_a_second.max(1.0);
        parts.push(format!("{pace:.1}× as fast as it plays"));
    }
    if let Some(left) = progress.time_left() {
        parts.push(format!("about {} left", seconds_text(left)));
    }
    if let Some(encoder) = &progress.encoder {
        parts.push(encoder_text(encoder));
    }
    parts.join(" · ")
}

fn done_text(done: &Exported) -> String {
    let took = done.took.as_secs_f64().max(0.001);
    format!(
        "Saved {} of video in {}, {:.1}× as fast as it plays, {}. {}",
        seconds_text(done.length),
        seconds_text(done.took),
        done.length.as_secs_f64() / took,
        encoder_text(&done.encoder),
        sound_text(done.loudness, done.true_peak)
    )
}

/// What the exported sound measured: its integrated loudness and highest
/// true peak, with a word when the peak is over the usual ceiling.
fn sound_text(loudness: f32, true_peak: f32) -> String {
    if !loudness.is_finite() {
        return "The sound is silent.".to_owned();
    }
    let over = if true_peak > ui_kit::master::TRUE_PEAK_CEILING {
        format!(
            " (over {:.0} dBTP: it may distort when played or encoded again)",
            ui_kit::master::TRUE_PEAK_CEILING
        )
    } else {
        String::new()
    };
    format!(
        "Loudness {} LUFS integrated, true peak {} dBTP{over}.",
        ui_kit::master::db_text(loudness),
        ui_kit::master::db_text(true_peak)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_file_is_named_after_the_project_and_ends_in_mp4() {
        let project = Path::new("/videos/projects/Weekly highlights.project");
        assert_eq!(
            default_path(project, "Weekly highlights"),
            PathBuf::from("/videos/projects/Weekly highlights.mp4")
        );
        // Characters a file name can't have are replaced; an empty name gets one.
        assert_eq!(
            default_path(project, "Cut 2: the \"best\" bits?"),
            PathBuf::from("/videos/projects/Cut 2- the -best- bits-.mp4")
        );
        assert_eq!(
            default_path(project, "  "),
            PathBuf::from("/videos/projects/Video.mp4")
        );
        assert_eq!(with_mp4(PathBuf::from("/a/clip")), PathBuf::from("/a/clip.mp4"));
        assert_eq!(
            with_mp4(PathBuf::from("/a/clip.MP4")),
            PathBuf::from("/a/clip.MP4")
        );
        assert_eq!(
            with_mp4(PathBuf::from("/a/clip.v2")),
            PathBuf::from("/a/clip.v2.mp4")
        );
    }

    #[test]
    fn sizes_and_times_are_said_plainly() {
        // Ten minutes at 12 megabits a second, with sound: about 914 MB.
        let size = megabytes(12_000_000, Time::seconds(600));
        assert!((size - 914.4).abs() < 0.1, "{size}");
        assert_eq!(megabytes_text(size), "914 MB");
        assert_eq!(megabytes_text(2500.0), "2.5 GB");
        assert_eq!(megabytes_text(3.21), "3.2 MB");
        assert_eq!(short(60.0), "60");
        assert_eq!(short(59.94006), "59.94");
        assert_eq!(seconds_text(Duration::from_secs(42)), "42 s");
        assert_eq!(seconds_text(Duration::from_secs(135)), "2 min 15 s");
    }

    #[test]
    fn progress_reads_as_a_sentence() {
        let progress = Progress {
            frames_done: 600,
            frames: 3600,
            elapsed: Duration::from_secs(5),
            encoder: Some("h264_amf".to_owned()),
        };
        assert_eq!(
            progress_text(&progress, 60.0),
            "Frame 600 of 3600 · 2.0× as fast as it plays · about 25 s left · encoded on the graphics card (h264_amf)"
        );
        let starting = Progress {
            frames_done: 0,
            frames: 3600,
            elapsed: Duration::ZERO,
            encoder: None,
        };
        assert_eq!(progress_text(&starting, 60.0), "Frame 0 of 3600");
        let done = Exported {
            path: PathBuf::from("/a/b.mp4"),
            frames: 3600,
            took: Duration::from_secs(20),
            length: Duration::from_secs(60),
            encoder: "libx264".to_owned(),
            on_graphics_card: false,
            loudness: -14.04,
            true_peak: -1.52,
        };
        assert_eq!(
            done_text(&done),
            "Saved 1 min 0 s of video in 20 s, 3.0× as fast as it plays, encoded on the processor. Loudness -14.0 LUFS integrated, true peak -1.5 dBTP."
        );
        assert_eq!(
            sound_text(f32::NEG_INFINITY, f32::NEG_INFINITY),
            "The sound is silent."
        );
        assert!(sound_text(-9.0, 0.4).contains("over -1 dBTP"));
    }

    #[test]
    fn a_folder_that_is_not_there_is_said_before_the_work_starts() {
        let dir = tempfile::tempdir().unwrap();
        assert!(prepare(&dir.path().join("out.mp4")).is_ok());
        let err = prepare(&dir.path().join("missing").join("out.mp4")).unwrap_err();
        assert!(err.report().contains("does not exist"));
    }
}
