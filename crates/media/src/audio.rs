//! Turning a sound stream into samples: stereo, 32-bit float, at the rate
//! the sound card plays, from any point of the file.
//!
//! [`AudioDecoder::seek`] goes to a time; [`AudioDecoder::read`] then fills
//! a buffer with the samples from there on. Like pictures, sound is placed
//! by its timestamps, so it stays in step with the picture over long files.
//!
//! A seek searches by the file's picture stream when it has one. Recordings
//! often list where to jump to for the picture only; searching by a sound
//! stream then makes FFmpeg read the file from where it last was up to the
//! target, which takes many seconds far into a long recording. The search
//! lands a little before the target and the sound up to it is dropped, so
//! the first sample read is the one asked for.

use crate::ffi::{Input, error_text};
use diag::AppError;
use ffmpeg_sys_next as ff;
use std::collections::VecDeque;
use std::path::Path;
use std::ptr;

/// Samples come out as interleaved stereo.
pub const CHANNELS: usize = 2;

/// How far before the target a seek asks the file to go (seconds). Sound
/// and picture for the same moment sit close together in a file but not in
/// a fixed order; this covers the difference.
const SEEK_MARGIN: f64 = 0.5;

pub struct AudioDecoder {
    input: Input,
    codec: *mut ff::AVCodecContext,
    stream: i32,
    /// The stream a seek searches by, and its time base: the picture when
    /// the file has one, else the sound itself.
    seek_stream: i32,
    seek_time_base: f64,
    time_base: f64,
    start: f64,
    rate: u32,
    resampler: *mut ff::SwrContext,
    frame: *mut ff::AVFrame,
    packet: *mut ff::AVPacket,
    /// Decoded, converted samples waiting to be read (interleaved stereo).
    pending: VecDeque<f32>,
    /// File time of the first sample in `pending`.
    pending_time: f64,
    /// After a seek: samples before this time are dropped.
    skip_until: Option<f64>,
    /// Sample frames of silence to give before `pending`: the sound in the
    /// file starts this much after the time that was asked for.
    silence: usize,
    ended: bool,
}

// One thread at a time uses a decoder; FFmpeg contexts may move between threads.
unsafe impl Send for AudioDecoder {}

impl std::fmt::Debug for AudioDecoder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "AudioDecoder(stream {}, at {:.3} s)",
            self.stream,
            self.position()
        )
    }
}

fn failed(path: &Path, what: &str, code: i32) -> AppError {
    AppError::new("The sound can't be played")
        .why(format!(
            "{what} failed for {} ({}).",
            path.display(),
            error_text(code)
        ))
        .fix("Check that the file still plays in a media player. If it does, report it with the file.")
}

impl AudioDecoder {
    /// Opens sound stream `stream` of the file, to be read at `rate` samples per second.
    pub fn open(path: &Path, stream: u32, rate: u32) -> diag::Result<AudioDecoder> {
        let input = Input::open(path).map_err(|e| {
            AppError::new("The sound can't be played")
                .why(format!("{}: {}.", path.display(), e.text))
                .fix("Check that the file still exists and that the drive is connected.")
        })?;
        let raw = input.raw();
        // SAFETY: `raw` is an opened context owned by `input`; pointers from
        // FFmpeg are checked before use; contexts are freed in Drop.
        unsafe {
            if stream >= (*raw).nb_streams {
                return Err(AppError::new("The sound can't be played")
                    .why(format!("{} has no stream {stream}.", path.display()))
                    .fix("Import the file again."));
            }
            let st = *(*raw).streams.add(stream as usize);
            let par = (*st).codecpar;
            let decoder = ff::avcodec_find_decoder((*par).codec_id);
            if decoder.is_null() {
                return Err(AppError::new("The sound can't be played")
                    .why(format!(
                        "No decoder for this kind of sound in {}.",
                        path.display()
                    ))
                    .fix("Convert the file's sound to AAC, Opus or WAV first."));
            }
            let codec = ff::avcodec_alloc_context3(decoder);
            let time_base = |st: *mut ff::AVStream| {
                f64::from((*st).time_base.num) / f64::from((*st).time_base.den.max(1))
            };
            let seek_stream = picture_stream(raw).unwrap_or(stream);
            let mut this = AudioDecoder {
                input,
                codec,
                stream: stream as i32,
                seek_stream: seek_stream as i32,
                seek_time_base: time_base(*(*raw).streams.add(seek_stream as usize)),
                time_base: time_base(st),
                start: 0.0,
                rate: rate.max(8_000),
                resampler: ptr::null_mut(),
                frame: ff::av_frame_alloc(),
                packet: ff::av_packet_alloc(),
                pending: VecDeque::new(),
                pending_time: 0.0,
                skip_until: None,
                silence: 0,
                ended: false,
            };
            if codec.is_null() {
                return Err(failed(path, "Setting up the decoder", ff::AVERROR(ff::ENOMEM)));
            }
            if (*st).start_time != ff::AV_NOPTS_VALUE {
                this.start = (*st).start_time as f64 * this.time_base;
            }
            let r = ff::avcodec_parameters_to_context(codec, par);
            if r < 0 {
                return Err(failed(path, "Reading the sound settings", r));
            }
            let r = ff::avcodec_open2(codec, decoder, ptr::null_mut());
            if r < 0 {
                return Err(failed(path, "Opening the decoder", r));
            }
            Ok(this)
        }
    }

    /// File time of the next sample `read` returns.
    pub fn position(&self) -> f64 {
        self.pending_time
    }

    /// Goes to `seconds` from the start of the file. The next sample read
    /// is the one at that time; where the file has no sound there (it starts
    /// later, or has a hole), silence comes first.
    pub fn seek(&mut self, seconds: f64) {
        let target = seconds.max(0.0);
        let from = target + self.start - SEEK_MARGIN;
        // SAFETY: valid contexts owned by self; the resampler is freed and
        // made again for the next frame so no old samples leak through.
        unsafe {
            let raw = self.input.raw();
            let mut found = ff::av_seek_frame(
                raw,
                self.seek_stream,
                (from / self.seek_time_base) as i64,
                ff::AVSEEK_FLAG_BACKWARD,
            );
            if found < 0 && self.seek_stream != self.stream {
                // The picture can't be searched by: the sound's own stream.
                found = ff::av_seek_frame(
                    raw,
                    self.stream,
                    (from / self.time_base) as i64,
                    ff::AVSEEK_FLAG_BACKWARD,
                );
            }
            if found < 0 {
                tracing::warn!(
                    "the sound could not jump to {target:.3} s ({}); it carries on from where it was",
                    error_text(found)
                );
            }
            ff::avcodec_flush_buffers(self.codec);
            ff::swr_free(&mut self.resampler);
        }
        self.pending.clear();
        self.pending_time = target;
        self.skip_until = Some(target);
        self.silence = 0;
        self.ended = false;
    }

    /// Fills `out` (interleaved stereo) with the next samples. Returns how
    /// many sample frames were written; fewer than asked means the end.
    pub fn read(&mut self, out: &mut [f32]) -> usize {
        let wanted = out.len() / CHANNELS;
        while self.silence + self.pending.len() / CHANNELS < wanted && !self.ended {
            if let Err(err) = self.decode_more() {
                tracing::warn!("{}", err.report());
                self.ended = true;
            }
        }
        let quiet = wanted.min(self.silence);
        self.silence -= quiet;
        let (silent, sounding) = out.split_at_mut(quiet * CHANNELS);
        silent.fill(0.0);
        let frames = (wanted - quiet).min(self.pending.len() / CHANNELS);
        for (o, s) in sounding.iter_mut().zip(self.pending.drain(..frames * CHANNELS)) {
            *o = s;
        }
        self.pending_time += (quiet + frames) as f64 / f64::from(self.rate);
        quiet + frames
    }

    /// Decodes one more frame and converts it into `pending`.
    fn decode_more(&mut self) -> diag::Result<()> {
        // SAFETY: send/receive contract followed; packets unreferenced after
        // use; the frame is unreferenced after converting.
        unsafe {
            loop {
                let r = ff::avcodec_receive_frame(self.codec, self.frame);
                if r == 0 {
                    let result = self.convert();
                    ff::av_frame_unref(self.frame);
                    return result;
                }
                if r == ff::AVERROR_EOF {
                    self.ended = true;
                    return Ok(());
                }
                if r != ff::AVERROR(ff::EAGAIN) {
                    return Err(AppError::new("The sound can't be played")
                        .why(format!("Decoding failed ({}).", error_text(r))));
                }
                loop {
                    let r = ff::av_read_frame(self.input.raw(), self.packet);
                    if r < 0 {
                        ff::avcodec_send_packet(self.codec, ptr::null());
                        break;
                    }
                    if (*self.packet).stream_index == self.stream {
                        let r = ff::avcodec_send_packet(self.codec, self.packet);
                        ff::av_packet_unref(self.packet);
                        if r < 0 && r != ff::AVERROR(ff::EAGAIN) {
                            continue; // a broken packet: skip it
                        }
                        break;
                    }
                    ff::av_packet_unref(self.packet);
                }
            }
        }
    }

    unsafe fn convert(&mut self) -> diag::Result<()> {
        // SAFETY: `frame` holds a decoded audio frame; the resampler is made
        // for its layout; the output buffer is sized for the converted count.
        unsafe {
            let f = &*self.frame;
            if self.resampler.is_null() {
                let mut out_layout: ff::AVChannelLayout = std::mem::zeroed();
                ff::av_channel_layout_default(&mut out_layout, CHANNELS as i32);
                let r = ff::swr_alloc_set_opts2(
                    &mut self.resampler,
                    &out_layout,
                    ff::AVSampleFormat::AV_SAMPLE_FMT_FLT,
                    self.rate as i32,
                    &f.ch_layout,
                    std::mem::transmute::<i32, ff::AVSampleFormat>(f.format),
                    f.sample_rate,
                    0,
                    ptr::null_mut(),
                );
                ff::av_channel_layout_uninit(&mut out_layout);
                if r < 0 || ff::swr_init(self.resampler) < 0 {
                    return Err(AppError::new("The sound can't be played").why(format!(
                        "Converting its sound for playback failed ({}).",
                        error_text(r)
                    )));
                }
            }
            let capacity = (i64::from(f.nb_samples) * i64::from(self.rate) / i64::from(f.sample_rate.max(1))
                + 256) as usize;
            let mut buf = vec![0f32; capacity * CHANNELS];
            let out = [buf.as_mut_ptr() as *mut u8];
            let n = ff::swr_convert(
                self.resampler,
                out.as_ptr(),
                capacity as i32,
                f.extended_data as *const *const u8,
                f.nb_samples,
            );
            if n <= 0 {
                return Ok(());
            }
            buf.truncate(n as usize * CHANNELS);
            let ts = f.best_effort_timestamp;
            let frame_time = if ts == ff::AV_NOPTS_VALUE {
                self.pending_time + self.pending.len() as f64 / (CHANNELS as f64 * f64::from(self.rate))
            } else {
                ts as f64 * self.time_base - self.start
            };
            let mut skip = 0usize;
            if let Some(until) = self.skip_until {
                match place(until, frame_time, n as usize, self.rate) {
                    Placed::Before => return Ok(()),
                    Placed::Across { drop } => skip = drop,
                    Placed::After { silence } => self.silence = silence,
                }
                self.skip_until = None;
            } else if self.pending.is_empty() && self.silence == 0 {
                self.pending_time = frame_time;
            }
            self.pending.extend(&buf[skip * CHANNELS..]);
            Ok(())
        }
    }
}

/// Where a decoded frame lies against the time a seek asked for.
#[derive(Debug, PartialEq, Eq)]
enum Placed {
    /// All of it is before the time: dropped.
    Before,
    /// The time is inside it: this many sample frames are dropped.
    Across { drop: usize },
    /// It starts after the time: this much silence comes first.
    After { silence: usize },
}

/// After a seek the position stays the time that was asked for: sound
/// before it is dropped, and sound that starts after it is preceded by
/// silence. `frames` is the length of the decoded frame at `rate`.
fn place(asked: f64, frame_time: f64, frames: usize, rate: u32) -> Placed {
    let early = ((asked - frame_time) * f64::from(rate)).round();
    if early >= frames as f64 {
        Placed::Before
    } else if early >= 0.0 {
        Placed::Across { drop: early as usize }
    } else {
        Placed::After {
            silence: (-early) as usize,
        }
    }
}

/// The file's first picture stream that is a real one (not a cover image).
///
/// # Safety
/// `raw` must be an opened format context.
unsafe fn picture_stream(raw: *mut ff::AVFormatContext) -> Option<u32> {
    // SAFETY: the streams array has `nb_streams` valid entries, each with
    // codec parameters, for as long as the context is open.
    unsafe {
        (0..(*raw).nb_streams).find(|&i| {
            let st = *(*raw).streams.add(i as usize);
            (*(*st).codecpar).codec_type == ff::AVMediaType::AVMEDIA_TYPE_VIDEO
                && (*st).disposition & ff::AV_DISPOSITION_ATTACHED_PIC == 0
        })
    }
}

impl Drop for AudioDecoder {
    fn drop(&mut self) {
        // SAFETY: each pointer was allocated by FFmpeg and is freed once.
        unsafe {
            ff::swr_free(&mut self.resampler);
            ff::av_frame_free(&mut self.frame);
            ff::av_packet_free(&mut self.packet);
            ff::avcodec_free_context(&mut self.codec);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(name)
    }

    #[test]
    fn a_seek_searches_by_the_picture_when_the_file_has_one() {
        let recording = AudioDecoder::open(&fixture("game-3-audio.mkv"), 2, 48_000).unwrap();
        assert_eq!((recording.stream, recording.seek_stream), (2, 0));
        let sound_only = AudioDecoder::open(&fixture("voice.mp3"), 0, 48_000).unwrap();
        assert_eq!((sound_only.stream, sound_only.seek_stream), (0, 0));
    }

    #[test]
    fn sound_is_placed_at_the_time_asked_for() {
        // A frame of 1024 samples at 48 kHz lasts 21.3 ms.
        assert_eq!(place(10.0, 9.9, 1024, 48_000), Placed::Before);
        assert_eq!(place(10.0, 10.0, 1024, 48_000), Placed::Across { drop: 0 });
        assert_eq!(place(10.0, 9.99, 1024, 48_000), Placed::Across { drop: 480 });
        assert_eq!(place(10.0, 10.011, 1024, 48_000), Placed::After { silence: 528 });
        assert_eq!(
            place(10.0, 70.0, 1024, 48_000),
            Placed::After { silence: 2_880_000 }
        );
    }
}
