//! Turning a sound stream into samples: stereo, 32-bit float, at the rate
//! the sound card plays, from any point of the file.
//!
//! [`AudioDecoder::seek`] goes to a time; [`AudioDecoder::read`] then fills
//! a buffer with the samples from there on. Like pictures, sound is placed
//! by its timestamps, so it stays in step with the picture over long files.

use crate::ffi::{Input, error_text};
use diag::AppError;
use ffmpeg_sys_next as ff;
use std::collections::VecDeque;
use std::path::Path;
use std::ptr;

/// Samples come out as interleaved stereo.
pub const CHANNELS: usize = 2;

pub struct AudioDecoder {
    input: Input,
    codec: *mut ff::AVCodecContext,
    stream: i32,
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
            let mut this = AudioDecoder {
                input,
                codec,
                stream: stream as i32,
                time_base: f64::from((*st).time_base.num) / f64::from((*st).time_base.den.max(1)),
                start: 0.0,
                rate: rate.max(8_000),
                resampler: ptr::null_mut(),
                frame: ff::av_frame_alloc(),
                packet: ff::av_packet_alloc(),
                pending: VecDeque::new(),
                pending_time: 0.0,
                skip_until: None,
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

    /// Goes to `seconds` from the start of the file.
    pub fn seek(&mut self, seconds: f64) {
        let target = seconds.max(0.0);
        let ts = ((target + self.start) / self.time_base) as i64;
        // SAFETY: valid contexts owned by self; the resampler is freed and
        // made again for the next frame so no old samples leak through.
        unsafe {
            ff::av_seek_frame(self.input.raw(), self.stream, ts, ff::AVSEEK_FLAG_BACKWARD);
            ff::avcodec_flush_buffers(self.codec);
            ff::swr_free(&mut self.resampler);
        }
        self.pending.clear();
        self.pending_time = target;
        self.skip_until = Some(target);
        self.ended = false;
    }

    /// Fills `out` (interleaved stereo) with the next samples. Returns how
    /// many sample frames were written; fewer than asked means the end.
    pub fn read(&mut self, out: &mut [f32]) -> usize {
        let wanted = out.len() / CHANNELS;
        while self.pending.len() < wanted * CHANNELS && !self.ended {
            if let Err(err) = self.decode_more() {
                tracing::warn!("{}", err.report());
                self.ended = true;
            }
        }
        let frames = wanted.min(self.pending.len() / CHANNELS);
        for (o, s) in out.iter_mut().zip(self.pending.drain(..frames * CHANNELS)) {
            *o = s;
        }
        self.pending_time += frames as f64 / f64::from(self.rate);
        frames
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
                skip =
                    (((until - frame_time) * f64::from(self.rate)).round().max(0.0) as usize).min(n as usize);
                if skip < n as usize {
                    self.skip_until = None;
                    if self.pending.is_empty() {
                        self.pending_time = frame_time + skip as f64 / f64::from(self.rate);
                    }
                }
            } else if self.pending.is_empty() {
                self.pending_time = frame_time;
            }
            if self.skip_until.is_none() {
                self.pending.extend(&buf[skip * CHANNELS..]);
            }
            Ok(())
        }
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
