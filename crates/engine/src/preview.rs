//! The preview worker: decodes the picture at the playhead on its own thread.
//!
//! The interface calls [`Preview::show`] whenever the playhead moves; only the
//! newest request counts, so scrubbing never builds a queue. Finished pictures
//! come back through [`Preview::latest`], and a notifier (the window's repaint
//! request) runs when one is ready.
//!
//! Decoders are kept open for the files used most recently, so moving around
//! one recording only seeks.
//!
//! A picture comes back as the decoder's YUV planes when the viewer can
//! convert colour while drawing ([`Convert::WhileDrawing`]) and the file's
//! format allows; otherwise as RGBA, converted here on the worker.

use crate::timeline::video_at;
use media::{Decoded, Decoding, VideoDecoder};
use project::{MediaId, Project, SequenceId, Time};
use std::collections::VecDeque;
use std::sync::mpsc::{Receiver, Sender, TryRecvError, channel};
use std::time::{Duration, Instant};

/// How many files keep an open decoder.
const OPEN_DECODERS: usize = 4;

/// Where a picture's colour is converted for the screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Convert {
    /// The viewer's shader converts the decoder's planes while drawing.
    /// Formats that can't go that way still come as RGBA, and so do planes
    /// with a side longer than `largest_side` (the most a texture can be).
    WhileDrawing { largest_side: u32 },
    /// Every picture is converted to RGBA on the worker.
    OnCpu,
}

/// A picture for one moment of the timeline.
#[derive(Debug)]
pub struct Picture {
    /// The timeline time asked for.
    pub at: Time,
    /// `None` in a gap (black), or when the frame couldn't be decoded (see `error`).
    pub frame: Option<Decoded>,
    pub error: Option<diag::AppError>,
    /// How long decoding took, with copying the planes out or converting to RGBA.
    pub took: Duration,
    /// Which decoder made it, for the viewer's corner.
    pub method: Option<String>,
}

struct Request {
    project: Project,
    sequence: SequenceId,
    at: Time,
    max_width: u32,
    max_height: u32,
}

pub struct Preview {
    requests: Sender<Request>,
    pictures: Receiver<Picture>,
}

impl std::fmt::Debug for Preview {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Preview")
    }
}

impl Preview {
    /// Starts the worker. `notify` runs on the worker each time a picture is ready.
    pub fn start(decoding: Decoding, convert: Convert, notify: impl Fn() + Send + 'static) -> Preview {
        let (requests, inbox) = channel::<Request>();
        let (outbox, pictures) = channel::<Picture>();
        let spawned = std::thread::Builder::new()
            .name("preview".into())
            .spawn(move || run(&inbox, &outbox, decoding, convert, &notify));
        if let Err(err) = spawned {
            tracing::error!("the preview worker could not start: {err}");
        }
        Preview { requests, pictures }
    }

    /// Asks for the picture at `at` on `sequence`, fitted in `max_width × max_height`
    /// (planes come at the nearest halving that still fills it).
    /// Replaces any request not started yet.
    pub fn show(&self, project: &Project, sequence: SequenceId, at: Time, max_width: u32, max_height: u32) {
        let _ = self.requests.send(Request {
            project: project.clone(),
            sequence,
            at,
            max_width,
            max_height,
        });
    }

    /// The newest finished picture, if any arrived since the last call.
    pub fn latest(&self) -> Option<Picture> {
        let mut newest = None;
        while let Ok(p) = self.pictures.try_recv() {
            newest = Some(p);
        }
        newest
    }
}

fn run(
    inbox: &Receiver<Request>,
    outbox: &Sender<Picture>,
    decoding: Decoding,
    convert: Convert,
    notify: &dyn Fn(),
) {
    let mut decoders: VecDeque<((MediaId, u32), VideoDecoder)> = VecDeque::new();
    while let Ok(mut request) = inbox.recv() {
        // Only the newest request counts.
        loop {
            match inbox.try_recv() {
                Ok(newer) => request = newer,
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => return,
            }
        }
        let picture = picture(&request, decoding, convert, &mut decoders);
        if outbox.send(picture).is_err() {
            return;
        }
        notify();
    }
}

fn picture(
    r: &Request,
    decoding: Decoding,
    convert: Convert,
    decoders: &mut VecDeque<((MediaId, u32), VideoDecoder)>,
) -> Picture {
    let started = Instant::now();
    let mut out = Picture {
        at: r.at,
        frame: None,
        error: None,
        took: Duration::ZERO,
        method: None,
    };
    let Some(sequence) = r.project.sequences.iter().find(|s| s.id == r.sequence) else {
        return out;
    };
    let Some(showing) = video_at(&r.project, sequence, r.at) else {
        return out; // a gap: black
    };
    let key = (showing.media.id, showing.clip.stream);
    let rotation = showing.media.info.video.as_ref().map_or(0, |v| v.rotation);
    let index = match decoders.iter().position(|(k, _)| *k == key) {
        Some(i) => i,
        None => match VideoDecoder::open(&showing.media.path, showing.clip.stream, rotation, decoding) {
            Ok(d) => {
                tracing::info!(file = %showing.media.path.display(), method = %d.method, "opened video");
                decoders.push_front((key, d));
                decoders.truncate(OPEN_DECODERS);
                0
            }
            Err(err) => {
                out.error = Some(err);
                return out;
            }
        },
    };
    // Most recently used first.
    if let Some(entry) = decoders.remove(index) {
        decoders.push_front(entry);
    }
    if let Some((_, decoder)) = decoders.front_mut() {
        let at = showing.source_time.as_secs_f64();
        let decode = |decoder: &mut VideoDecoder| match convert {
            Convert::WhileDrawing { largest_side } => {
                match decoder.picture_at(at, r.max_width, r.max_height)? {
                    Decoded::Yuv(planes) if planes.width.max(planes.height) > largest_side => {
                        decoder.frame_at(at, r.max_width, r.max_height).map(Decoded::Rgba)
                    }
                    picture => Ok(picture),
                }
            }
            Convert::OnCpu => decoder.frame_at(at, r.max_width, r.max_height).map(Decoded::Rgba),
        };
        match decode(decoder) {
            Ok(frame) => out.frame = Some(frame),
            // The GPU couldn't decode this file (an unusual profile, a driver
            // problem): the CPU takes over for this file from now on.
            Err(err) if decoder.method.starts_with("hardware") => {
                tracing::warn!(file = %showing.media.path.display(), "hardware decoding failed, using software: {}", err.report());
                match VideoDecoder::open(
                    &showing.media.path,
                    showing.clip.stream,
                    rotation,
                    Decoding::Software,
                ) {
                    Ok(mut software) => {
                        match decode(&mut software) {
                            Ok(frame) => out.frame = Some(frame),
                            Err(err) => out.error = Some(err),
                        }
                        *decoder = software;
                    }
                    Err(_) => out.error = Some(err),
                }
            }
            Err(err) => out.error = Some(err),
        }
        out.method = Some(decoder.method.clone());
    }
    out.took = started.elapsed();
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use project::Command;
    use std::path::Path;

    const PLANES: Convert = Convert::WhileDrawing { largest_side: 8192 };

    fn fixture(name: &str) -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../media/tests/fixtures")
            .join(name)
    }

    fn wait(preview: &Preview) -> Picture {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            if let Some(p) = preview.latest() {
                return p;
            }
            assert!(Instant::now() < deadline, "no picture in time");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn pictures_for_clips_and_black_for_gaps() {
        let project = Project::new_automatic("T");
        let seq = project.active_sequence;
        let media = media::describe(&fixture("game-3-audio.mkv")).unwrap();
        let project = Command::Import { sequence: seq, media }.apply(&project).unwrap();
        let preview = Preview::start(Decoding::Software, Convert::OnCpu, || {});

        preview.show(&project, seq, Time::seconds(1), 160, 160);
        let p = wait(&preview);
        let frame = p.frame.expect("a picture");
        assert!(matches!(frame, Decoded::Rgba(_)));
        assert_eq!(frame.size(), (160, 90));
        assert!((frame.time() - 1.0).abs() < 0.02);
        assert!(p.error.is_none());

        preview.show(&project, seq, Time::seconds(30), 160, 160);
        let gap = wait(&preview);
        assert!(
            gap.frame.is_none() && gap.error.is_none(),
            "after the last clip is black"
        );
    }

    #[test]
    fn planes_for_the_viewer_to_convert() {
        let project = Project::new_automatic("T");
        let seq = project.active_sequence;
        let media = media::describe(&fixture("game-3-audio.mkv")).unwrap();
        let project = Command::Import { sequence: seq, media }.apply(&project).unwrap();
        let preview = Preview::start(Decoding::Software, PLANES, || {});

        preview.show(&project, seq, Time::seconds(1), 320, 320);
        let p = wait(&preview);
        let Some(Decoded::Yuv(frame)) = p.frame else {
            panic!("planes, got {:?}", p.frame);
        };
        assert_eq!((frame.width, frame.height), (320, 180));
        assert!((frame.time - 1.0).abs() < 0.02);
        // A smaller viewer gets the planes halved.
        preview.show(&project, seq, Time::seconds(1), 160, 160);
        assert_eq!(wait(&preview).frame.expect("a picture").size(), (160, 90));
    }

    #[test]
    fn planes_too_big_for_a_texture_come_as_rgba() {
        let project = Project::new_automatic("T");
        let seq = project.active_sequence;
        let media = media::describe(&fixture("game-3-audio.mkv")).unwrap();
        let project = Command::Import { sequence: seq, media }.apply(&project).unwrap();
        let small = Convert::WhileDrawing { largest_side: 256 };
        let preview = Preview::start(Decoding::Software, small, || {});
        preview.show(&project, seq, Time::seconds(1), 320, 320);
        let frame = wait(&preview).frame.expect("a picture");
        assert!(matches!(frame, Decoded::Rgba(_)), "{frame:?}");
        assert_eq!(frame.size(), (320, 180));
    }

    #[test]
    fn a_missing_file_is_explained() {
        let project = Project::new_automatic("T");
        let seq = project.active_sequence;
        let mut media = media::describe(&fixture("game-3-audio.mkv")).unwrap();
        media.path = fixture("moved-away.mkv");
        let project = Command::Import { sequence: seq, media }.apply(&project).unwrap();
        let preview = Preview::start(Decoding::Software, PLANES, || {});
        preview.show(&project, seq, Time::seconds(1), 160, 160);
        let p = wait(&preview);
        assert!(p.frame.is_none());
        assert!(p.error.unwrap().what().contains("can't be shown"));
    }
}
