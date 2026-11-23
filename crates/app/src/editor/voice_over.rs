//! Recording a voice-over: starting a take, rolling the timeline once the
//! microphone gives sound, stopping, and placing the finished take
//! (`crate::voice_over` has the parts that don't need the editor).

use super::Editor;
use crate::voice_over::{self, Phase, Pick, Take};
use diag::AppError;
use egui::{Align2, Color32, FontId, Rect, Sense, Stroke, pos2, vec2};
use project::{Command, TrackId, TrackKind};
use std::time::Duration;

impl Editor {
    /// Shift+R, or "Record a voice-over here" on track `asked`: starts a
    /// take at the playhead, or stops the one under way.
    pub(super) fn record_voice_over(&mut self, asked: Option<TrackId>) -> diag::Result<()> {
        if self.voice_over.is_some() {
            self.stop_voice_over();
            return Ok(());
        }
        let Some(document) = &self.document else {
            return Err(AppError::new("No project is open")
                .why("A voice-over goes on a timeline.")
                .fix("Open a project from the launcher."));
        };
        let project_file = document.path().to_path_buf();
        let seq = document.project().active().ok_or_else(|| {
            AppError::new("There is no timeline to record on").fix("Make a timeline, then record again.")
        })?;
        let sequence = seq.id;
        let track = match voice_over::pick(seq, asked, self.timeline.target_audio) {
            Pick::Track(track) => track,
            Pick::Make => {
                self.with_document(|d| {
                    d.apply(Command::AddTrack {
                        sequence,
                        kind: TrackKind::Audio,
                        name: voice_over::TRACK_NAME.into(),
                    })
                })?;
                self.document
                    .as_ref()
                    .and_then(|d| d.project().active())
                    .and_then(voice_over::made_track)
                    .ok_or_else(|| AppError::new("The Voice-over track wasn't made"))?
            }
        };
        let track_name = self
            .document
            .as_ref()
            .and_then(|d| d.project().active())
            .and_then(|s| s.audio_tracks.iter().find(|t| t.id == track))
            .map_or_else(|| voice_over::TRACK_NAME.to_owned(), |t| t.name.clone());
        // The take starts at the playhead, from a standstill.
        if self.playback.is_some() {
            self.playback = None;
            self.after_play_change();
        }
        let at = self.snapped(self.viewer.playhead);
        let path = voice_over::take_path(&voice_over::folder(&project_file));
        let recording = engine::record::Recording::start(engine::record::Asked {
            microphone: self.settings.microphone.clone(),
            channel: self.settings.recording_channel.engine(),
            path,
        });
        self.status = format!("Opening the microphone to record on {track_name}…");
        self.voice_over = Some(Take {
            recording,
            sequence,
            track,
            at,
            phase: Phase::Opening,
            track_name,
        });
        Ok(())
    }

    /// Stops the take under way (it is then finished and placed).
    pub(super) fn stop_voice_over(&mut self) {
        let Some(take) = &mut self.voice_over else {
            return;
        };
        if matches!(take.phase, Phase::Opening | Phase::Counting(_) | Phase::Rolling) {
            take.recording.stop();
            take.phase = Phase::Finishing;
            self.status = "Finishing the voice-over…".into();
        }
        if self.playback.is_some() {
            self.playback = None;
            self.after_play_change();
        }
    }

    /// Once a frame while there is a take: rolls the timeline once the
    /// microphone gives sound, stops with playback, places what was recorded.
    pub(super) fn voice_over_poll(&mut self, ctx: &egui::Context) {
        let Some(take) = &mut self.voice_over else {
            return;
        };
        // The banner's clock and meter move on.
        ctx.request_repaint_after(Duration::from_millis(50));
        match &take.phase {
            Phase::Opening => {
                if let Some(Err(err)) = take.recording.finished() {
                    self.voice_over = None;
                    self.errors.push(&err);
                } else if take.recording.ready() {
                    let count_in = Duration::from_secs(self.settings.count_in.seconds());
                    if count_in.is_zero() {
                        self.roll_voice_over();
                    } else {
                        take.phase = Phase::Counting(std::time::Instant::now() + count_in);
                    }
                }
            }
            Phase::Counting(until) => {
                if let Some(Err(err)) = take.recording.finished() {
                    self.voice_over = None;
                    self.errors.push(&err);
                } else if std::time::Instant::now() >= *until {
                    self.roll_voice_over();
                }
            }
            Phase::Rolling => {
                // Play/pause, or the end of the timeline, stopped it; or
                // the microphone went away.
                if self.playback.is_none() {
                    self.stop_voice_over();
                }
            }
            Phase::Finishing => match take.recording.finished() {
                None => {}
                Some(Err(err)) => {
                    self.voice_over = None;
                    self.errors.push(&err);
                }
                Some(Ok(finished)) if finished.frames == 0 => {
                    self.voice_over = None;
                    let _ = std::fs::remove_file(&finished.path);
                    self.status = "Nothing was recorded.".into();
                }
                Some(Ok(finished)) => {
                    if finished.cut_short {
                        self.status = format!(
                            "The microphone stopped after {}: that much is kept.",
                            voice_over::clock(finished.seconds())
                        );
                    }
                    if finished.lost > 0 {
                        tracing::warn!("a voice-over lost {} samples: the disk was slow", finished.lost);
                    }
                    take.check(finished.path);
                }
            },
            Phase::Checking(rx) => {
                let Ok(checked) = rx.try_recv() else {
                    return;
                };
                let Some(take) = self.voice_over.take() else {
                    return;
                };
                let placed = match checked {
                    Ok(media) => {
                        let seconds = media.duration.as_secs_f64();
                        self.with_document(|d| {
                            d.apply(Command::PlaceRecording {
                                sequence: take.sequence,
                                media,
                                at: take.at,
                                track: take.track,
                            })
                        })
                        .map(|()| seconds)
                    }
                    Err(refused) => Err(refused.error),
                };
                match placed {
                    Ok(seconds) => {
                        self.status = format!(
                            "Voice-over of {} placed on {}. Undo takes it away.",
                            voice_over::clock(seconds),
                            take.track_name
                        );
                    }
                    Err(err) => self.errors.push(&err),
                }
            }
        }
    }

    /// The microphone gives sound: the timeline plays from where the take
    /// goes, and the take is kept from now on.
    fn roll_voice_over(&mut self) {
        let Some(take) = &mut self.voice_over else {
            return;
        };
        let Some(document) = &self.document else {
            return;
        };
        let project = document.project();
        let heard = voice_over::muted_while_recording(project, take.sequence, take.track);
        self.viewer.playhead = take.at;
        self.unheard_said = false;
        self.playback = Some(engine::Playback::start(
            heard.unwrap_or_else(|| project.clone()),
            take.sequence,
            take.at,
        ));
        take.recording.roll();
        take.phase = Phase::Rolling;
        self.status = format!(
            "Recording on {}: {} or play/pause stops",
            take.track_name,
            self.shortcuts
                .keys_or_name(ui_kit::shortcuts::Action::RecordVoiceOver)
        );
        self.after_play_change();
    }

    /// The banner over the window while a take is made: what it records
    /// on, how long, the microphone's level, and Stop.
    pub(super) fn voice_over_banner(&mut self, ctx: &egui::Context) {
        let Some(take) = &self.voice_over else {
            return;
        };
        let theme = self.theme;
        let (words, live) = match take.phase {
            Phase::Opening => (format!("Opening the microphone for {}…", take.track_name), false),
            Phase::Counting(until) => {
                let left = until.saturating_duration_since(std::time::Instant::now());
                (
                    format!("Recording on {} in {}…", take.track_name, left.as_secs() + 1),
                    false,
                )
            }
            Phase::Rolling => (
                format!(
                    "Recording on {} · {}",
                    take.track_name,
                    voice_over::clock(take.recording.seconds())
                ),
                true,
            ),
            Phase::Finishing | Phase::Checking(_) => ("Placing the voice-over…".to_owned(), false),
        };
        let level = take.recording.level();
        self.voice_over_peak = (self.voice_over_peak * 0.85).max(level);
        let shown = self.voice_over_peak;
        let mut stop = false;
        let screen = ctx.content_rect();
        egui::Area::new(egui::Id::unique("voice-over banner"))
            .order(egui::Order::Foreground)
            .fixed_pos(pos2(screen.center().x - 200.0, screen.top() + 52.0))
            .show(ctx, |ui| {
                let size = vec2(400.0, 34.0);
                let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
                let p = ui.painter();
                p.rect_filled(rect, 17.0, theme.panel);
                p.rect_stroke(rect, 17.0, Stroke::new(1.0, theme.line), egui::StrokeKind::Inside);
                // The red dot, steady while recording.
                let red = Color32::from_rgb(0xe5, 0x48, 0x4d);
                let dot = pos2(rect.left() + 18.0, rect.center().y);
                p.circle_filled(dot, 6.0, if live { red } else { red.gamma_multiply(0.4) });
                p.text(
                    pos2(rect.left() + 32.0, rect.center().y),
                    Align2::LEFT_CENTER,
                    &words,
                    FontId::proportional(13.0),
                    theme.text,
                );
                // The microphone's level: a bar, red near full scale.
                let lane =
                    Rect::from_min_size(pos2(rect.right() - 150.0, rect.center().y - 3.0), vec2(70.0, 6.0));
                p.rect_filled(lane, 3.0, theme.line);
                let db = 20.0 * shown.max(1e-6).log10();
                let share = ((db + 60.0) / 60.0).clamp(0.0, 1.0);
                let fill = if db > -3.0 { red } else { theme.accent };
                p.rect_filled(
                    Rect::from_min_size(lane.min, vec2(lane.width() * share, lane.height())),
                    3.0,
                    fill,
                );
                let button =
                    Rect::from_min_size(pos2(rect.right() - 70.0, rect.top() + 6.0), vec2(58.0, 22.0));
                let r = ui.interact(button, egui::Id::unique("voice-over stop"), Sense::click());
                p.rect_filled(
                    button,
                    11.0,
                    if r.hovered() {
                        theme.line_strong
                    } else {
                        theme.line
                    },
                );
                p.text(
                    button.center(),
                    Align2::CENTER_CENTER,
                    "Stop",
                    FontId::proportional(12.5),
                    theme.text,
                );
                stop = r.clicked();
            });
        if stop {
            self.stop_voice_over();
        }
    }
}
