//! The ways a file may be decoded, in order, and opening a file the
//! first way that works.

use super::fill::KEPT_BEHIND;
use super::{Convert, File, Worker};
use media::{Decoding, Level, VideoDecoder};
use project::MediaId;

/// The ways a file may be decoded, in the order they are tried. Decoding on
/// the window's device needs the device, the viewer's shader to draw from
/// it, and `Decoding::Best`.
pub fn ladder(decoding: Decoding, convert: Convert, on_device: bool) -> &'static [Level] {
    match (decoding, convert) {
        (Decoding::Best, Convert::WhileDrawing { .. }) if on_device => {
            &[Level::Device, Level::Hardware, Level::Software]
        }
        (Decoding::Best, _) => &[Level::Hardware, Level::Software],
        (Decoding::Software, _) => &[Level::Software],
    }
}

/// The ways left after `failed`: what a file steps down through.
pub fn below(ladder: &'static [Level], failed: Level) -> &'static [Level] {
    let first = ladder.iter().position(|l| *l > failed).unwrap_or(ladder.len());
    &ladder[first..]
}

impl Worker {
    /// The ways to try for a file: all of them, or those below the window's
    /// device once it has failed for this file.
    pub(super) fn ladder_for(&self, key: (MediaId, u32)) -> &'static [Level] {
        if self.off_device.contains(&key) {
            below(self.ladder, Level::Device)
        } else {
            self.ladder
        }
    }

    /// Opens the file the first way of `ways` that works for it. On the
    /// window's device that may fail for many reasons (a kind of video it
    /// doesn't decode, a driver problem): the next way is tried then. The
    /// other ways only fail when the file itself can't be opened.
    pub(super) fn open(
        &mut self,
        key: (MediaId, u32),
        file: &File<'_>,
        ways: &[Level],
    ) -> diag::Result<VideoDecoder> {
        let mut last = None;
        for way in ways {
            let opened = match (way, &self.on_device) {
                (Level::Device, Some(on_device)) => {
                    on_device.watch.about_to_decode();
                    let opened = VideoDecoder::open_on_device(
                        file.path,
                        file.stream,
                        file.rotation,
                        &on_device.device,
                    );
                    if opened.is_ok() {
                        on_device.watch.answered();
                    }
                    if let Err(err) = &opened {
                        self.off_device.insert(key);
                        on_device.watch.reported_an_error();
                        tracing::info!(file = %file.path.display(), "not decoded on the window's device: {}", err.report());
                    }
                    opened
                }
                (Level::Device, None) => continue,
                (Level::Hardware, _) => {
                    VideoDecoder::open(file.path, file.stream, file.rotation, Decoding::Best)
                }
                (Level::Software, _) => {
                    VideoDecoder::open(file.path, file.stream, file.rotation, Decoding::Software)
                }
            };
            match opened {
                Ok(mut decoder) => {
                    tracing::info!(file = %file.path.display(), method = %decoder.method, "opened video");
                    // Only what can be kept behind the playhead is worth a
                    // picture: a jump passes the rest without copying it.
                    decoder.keep_within(KEPT_BEHIND);
                    return Ok(decoder);
                }
                Err(err) => last = Some(err),
            }
        }
        Err(last.unwrap_or_else(|| {
            diag::AppError::new("The video can't be shown").why("No way of decoding it is left to try.")
        }))
    }
}
