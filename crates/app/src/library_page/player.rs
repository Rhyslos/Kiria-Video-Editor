//! The detail panel's small player: one file of the project, on its own.
//!
//! Its picture comes from a viewer of its own (`Viewer::second`, a worker
//! of its own on the window's devices, drawing from `SLOT`), asked for the
//! file alone (`engine::source::file_alone`), as the Edit page's split view
//! shows a clip raw. Its sound is the ordinary playback engine playing a
//! small project of that file alone (`listening_project`: the file
//! imported into a new timeline, every sound stream on its own track), so
//! what is heard is what the timeline would play; the playback clock moves
//! the picture. Play, pause, stop and stepping behave as on the Edit page:
//! pausing goes back to where playing began, Stop stays where it is.

use crate::viewer::Viewer;
use project::{Command, MediaId, MediaKind, MediaRef, Project, SequenceId};
use ui_kit::scrub::Marks;
use ui_kit::{PreviewQuality, Slot, Theme};

/// The picture slot the player draws from, beside the viewer's and the
/// split view's.
pub const SLOT: Slot = Slot(2);

/// A step for files without frames of their own (sound): a thirtieth of a second.
const SOUND_STEP: f64 = 1.0 / 30.0;

struct Playing {
    playback: engine::Playback,
    /// Where playing stops by itself: the out mark, or the file's end.
    stop_at: f64,
}

#[derive(Default)]
pub struct Player {
    /// The file on show, and where in it the player is (seconds).
    media: Option<MediaRef>,
    pub at: f64,
    playing: Option<Playing>,
    viewer: Option<Viewer>,
    /// The project of the file's picture alone, made once per file.
    alone: Option<(MediaId, Project)>,
    /// The project the file's sound is played from, made once per file.
    listening: Option<(MediaId, Project, SequenceId)>,
}

impl Player {
    /// Makes the player's viewer beside `main` the first time a picture is
    /// wanted; it costs nothing until then.
    pub fn ensure_viewer(&mut self, main: &Viewer) {
        if self.viewer.is_none() {
            self.viewer = Some(main.second(SLOT));
        }
    }

    pub fn media(&self) -> Option<MediaId> {
        self.media.as_ref().map(|m| m.id)
    }

    pub fn playing(&self) -> bool {
        self.playing.is_some()
    }

    /// Shows `media` (nothing for `None`): a new file stops playing and
    /// starts at its in mark, or its start.
    pub fn show(&mut self, media: Option<&MediaRef>, marks: Marks) {
        if self.media.as_ref().map(|m| m.id) == media.map(|m| m.id) {
            if let (Some(kept), Some(new)) = (&mut self.media, media) {
                // The same file, perhaps relinked: keep it current.
                if kept != new {
                    *kept = new.clone();
                }
            }
            return;
        }
        self.stop_here();
        self.media = media.cloned();
        self.at = marks.mark_in.unwrap_or(0.0);
        self.alone = None;
        self.listening = None;
    }

    /// The file's length in seconds.
    pub fn length(&self) -> f64 {
        self.media.as_ref().map_or(0.0, |m| m.duration.as_secs_f64())
    }

    /// Play from here, or pause (back to where playing began).
    pub fn play_pause(&mut self, marks: Marks) {
        match self.playing.take() {
            Some(playing) => {
                self.at = playing.playback.started_at().as_secs_f64();
                self.set_viewer_playing(false);
            }
            None => self.start(marks),
        }
    }

    /// Stops playing where it is.
    pub fn stop_here(&mut self) {
        if let Some(playing) = self.playing.take() {
            self.at = playing.playback.position().as_secs_f64().min(self.length());
            self.set_viewer_playing(false);
        }
    }

    /// Goes to `seconds` in the file; while playing, playing carries on from there.
    pub fn go_to(&mut self, seconds: f64, marks: Marks) {
        let was_playing = self.playing.is_some();
        self.stop_here();
        self.at = seconds.clamp(0.0, self.length());
        if was_playing {
            self.start(marks);
        }
    }

    /// One frame of the file back or forward (`frames` of them).
    pub fn step(&mut self, frames: i64, marks: Marks) {
        let frame = self
            .media
            .as_ref()
            .and_then(|m| m.info.video.as_ref())
            .map_or(SOUND_STEP, |v| v.frame_rate.frame_duration().as_secs_f64());
        let to = self.at + frames as f64 * frame;
        self.go_to(to, marks);
    }

    fn start(&mut self, marks: Marks) {
        let Some(media) = self.media.clone() else { return };
        let length = self.length();
        let (start, end) = marks.part(length);
        // Outside the marked part, or at its end: from its start.
        let from = if self.at < start || self.at >= end - 0.01 {
            start
        } else {
            self.at
        };
        if self.listening.as_ref().is_none_or(|(id, ..)| *id != media.id) {
            self.listening = listening_project(&media).map(|(p, s)| (media.id, p, s));
        }
        let Some((_, project, sequence)) = &self.listening else {
            return;
        };
        self.at = from;
        let playback = engine::Playback::start(project.clone(), *sequence, crate::editor::seconds(from));
        self.playing = Some(Playing {
            playback,
            stop_at: end,
        });
        self.set_viewer_playing(true);
    }

    fn set_viewer_playing(&mut self, on: bool) {
        let interval = self
            .media
            .as_ref()
            .and_then(|m| m.info.video.as_ref())
            .map(|v| v.frame_rate.frame_duration().as_secs_f64());
        if let Some(viewer) = &mut self.viewer {
            viewer.set_playing(interval.filter(|_| on));
        }
    }

    /// Every frame: follows the playback clock and stops at the out mark or
    /// the end; takes in the newest picture. Returns an error to show.
    pub fn poll(&mut self, ctx: &egui::Context, quality: PreviewQuality) -> Option<diag::AppError> {
        if let Some(playing) = &self.playing {
            let now = playing.playback.position().as_secs_f64();
            if now >= playing.stop_at {
                self.at = playing.stop_at;
                self.playing = None;
                self.set_viewer_playing(false);
            } else {
                self.at = now;
                ctx.request_repaint();
            }
        }
        self.viewer.as_mut()?.poll(ctx, quality)
    }

    /// Whether the picture last asked for has come back.
    pub fn settled(&self) -> bool {
        self.viewer.as_ref().is_none_or(Viewer::settled)
    }

    /// Draws the file's picture at the player's place, in `size`: the
    /// file's own frame, fitted. Only for files with a picture stream.
    pub fn picture_ui(
        &mut self,
        ui: &mut egui::Ui,
        theme: &Theme,
        size: egui::Vec2,
        quality: PreviewQuality,
    ) {
        let Some(media) = &self.media else { return };
        if self.alone.as_ref().is_none_or(|(id, _)| *id != media.id) {
            self.alone = engine::source::file_alone(media).map(|p| (media.id, p));
        }
        match (&mut self.viewer, &self.alone) {
            (Some(viewer), Some((_, alone))) => {
                viewer.source_ui(
                    ui,
                    theme,
                    size,
                    alone,
                    crate::editor::seconds(self.at),
                    quality,
                    "",
                );
            }
            _ => {
                ui_kit::viewer::empty(ui, theme, size, "No picture");
            }
        }
    }

    /// The app is closing: the viewer's worker finishes and ends.
    pub fn close(&mut self) {
        self.playing = None;
        if let Some(viewer) = &mut self.viewer {
            viewer.close();
        }
    }
}

/// A project of `media` alone, to play its sound: the file imported into a
/// new timeline as an import puts it there (its picture, and every sound
/// stream on a track of its own, from the start). `None` for a picture,
/// which has no sound.
pub fn listening_project(media: &MediaRef) -> Option<(Project, SequenceId)> {
    if media.kind == MediaKind::Image {
        return None;
    }
    let project = Project::new_automatic("Listen");
    let sequence = project.active_sequence;
    let project = Command::Import {
        sequence,
        media: media.clone(),
    }
    .apply(&project)
    .inspect_err(|err| tracing::info!("the file can't be played here: {}", err.report()))
    .ok()?;
    Some((project, sequence))
}

#[cfg(test)]
mod tests {
    use super::*;
    use project::{Fingerprint, Time};

    fn voice() -> MediaRef {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../media/tests/fixtures/voice.mp3");
        let probed = media::probe(&path).unwrap();
        MediaRef {
            id: MediaId(7),
            path,
            fingerprint: Fingerprint {
                size: 1,
                sample_hash: String::new(),
            },
            kind: probed.kind,
            duration: probed.duration,
            info: probed.info,
        }
    }

    #[test]
    fn a_sound_file_is_played_from_a_timeline_of_its_own() {
        let media = voice();
        let (project, sequence) = listening_project(&media).unwrap();
        let seq = project.sequence(sequence).unwrap();
        let clips: Vec<_> = seq.tracks().flat_map(|t| t.clips.iter()).collect();
        assert!(!clips.is_empty());
        assert!(
            clips.iter().all(|c| c.start == Time::ZERO),
            "the file's time is the timeline's"
        );
        assert_eq!(seq.length(), media.duration);
        let mut picture = media.clone();
        picture.kind = MediaKind::Image;
        assert!(listening_project(&picture).is_none());
    }

    #[test]
    fn pausing_goes_back_and_marks_bound_playing() {
        let media = voice();
        let mut player = Player::default();
        let marks = Marks {
            mark_in: Some(0.5),
            mark_out: Some(1.0),
        };
        player.show(Some(&media), marks);
        assert_eq!(player.at, 0.5, "a new file starts at its in mark");
        player.at = 3.0;
        player.play_pause(marks);
        assert!(player.playing());
        assert_eq!(player.at, 0.5, "outside the marks it plays from the in mark");
        player.play_pause(marks);
        assert!(!player.playing());
        assert_eq!(player.at, 0.5, "pausing goes back to where it began");
        player.go_to(99.0, marks);
        assert_eq!(player.at, media.duration.as_secs_f64(), "never past the end");
        player.step(-1, marks);
        assert!(player.at < media.duration.as_secs_f64());
        player.show(None, Marks::default());
        assert_eq!(player.media(), None);
    }
}
