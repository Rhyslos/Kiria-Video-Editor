//! Writing a video file: pictures and sound in, an MP4 with H.264 and AAC out.
//!
//! [`Writer`] takes pictures as NV12 (brightness at full size, then the two
//! colour samples interleaved at half size: what the compositor reads back)
//! and sound as interleaved stereo floats, encodes both and writes them
//! interleaved, so the file plays while it downloads.
//!
//! The picture is encoded on the graphics card when one of its encoders
//! opens, otherwise on the CPU ([`VIDEO_ENCODERS`], tried in order).

use crate::ffi::error_text;
use diag::AppError;
use ffmpeg_sys_next as ff;
use std::ffi::{CStr, CString};
use std::path::Path;
use std::ptr;

/// The H.264 encoders, in the order they are tried: the graphics cards'
/// own, then the system's, then the CPU's. (FFmpeg's names for them.)
pub const VIDEO_ENCODERS: [&str; 5] = ["h264_amf", "h264_nvenc", "h264_qsv", "h264_mf", "libx264"];
/// The one that needs no graphics card.
const SOFTWARE_ENCODER: &str = "libx264";
const CHANNELS: usize = 2;

/// What the picture stream is like.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VideoSettings {
    /// Both even: colour is stored at half size.
    pub width: u32,
    pub height: u32,
    /// Frames a second, as a fraction (60000/1001 for 59.94).
    pub rate: (u32, u32),
    /// Bits a second to aim for.
    pub bit_rate: u64,
    /// Encode on the CPU even when a graphics card could.
    pub software: bool,
}

/// What the sound stream is like: stereo at this rate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AudioSettings {
    pub sample_rate: u32,
    /// Bits a second.
    pub bit_rate: u64,
}

fn cannot(why: impl Into<String>) -> AppError {
    AppError::new("The video can't be exported")
        .why(why)
        .fix("Check that the folder can be written to and has room, then export again. If it keeps failing, choose the software encoder.")
}

fn failed(what: &str, code: i32) -> AppError {
    cannot(format!("{what} failed: {}.", error_text(code)))
}

/// One stream's encoder and where its packets go.
struct Track {
    codec: *mut ff::AVCodecContext,
    stream: *mut ff::AVStream,
    frame: *mut ff::AVFrame,
    /// Frames (pictures or samples) handed to the encoder so far.
    next: i64,
}

/// A file being written.
pub struct Writer {
    format: *mut ff::AVFormatContext,
    packet: *mut ff::AVPacket,
    video: Track,
    video_encoder: String,
    size: (u32, u32),
    audio: Option<Track>,
    /// Samples waiting for a full sound frame, interleaved.
    waiting: Vec<f32>,
}

// The contexts are only ever used through `&mut self`, one thread at a time.
// SAFETY: nothing in them is tied to the thread that made them.
unsafe impl Send for Writer {}

impl std::fmt::Debug for Writer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Writer({}×{} with {})",
            self.size.0, self.size.1, self.video_encoder
        )
    }
}

impl Writer {
    /// Starts the file at `path` (replacing one that is there), with sound
    /// when `audio` is given.
    pub fn create(path: &Path, video: &VideoSettings, audio: Option<&AudioSettings>) -> diag::Result<Writer> {
        if video.width == 0 || video.height == 0 || video.width % 2 != 0 || video.height % 2 != 0 {
            return Err(cannot(format!(
                "A picture of {}×{} can't be encoded: both sides must be even.",
                video.width, video.height
            )));
        }
        if video.rate.0 == 0 || video.rate.1 == 0 {
            return Err(cannot("The frame rate is not a number."));
        }
        let c_path = path
            .to_str()
            .and_then(|text| CString::new(text).ok())
            .ok_or_else(|| cannot("The file's name has characters that can't be passed on."))?;
        let mut format = ptr::null_mut();
        // SAFETY: `format` starts null so FFmpeg allocates it; the path is a
        // valid C string during the call; the container is taken from the
        // file's ending.
        let r = unsafe {
            ff::avformat_alloc_output_context2(&mut format, ptr::null(), ptr::null(), c_path.as_ptr())
        };
        if r < 0 || format.is_null() {
            return Err(failed("Starting the file", r));
        }
        // From here on `writer` owns everything made, and frees it when dropped.
        let mut writer = Writer {
            format,
            packet: ptr::null_mut(),
            video: Track::empty(),
            video_encoder: String::new(),
            size: (video.width, video.height),
            audio: None,
            waiting: Vec::new(),
        };
        // SAFETY: allocates a packet, checked for null below.
        writer.packet = unsafe { ff::av_packet_alloc() };
        if writer.packet.is_null() {
            return Err(cannot("There is no memory for it."));
        }
        let (track, name) = writer.open_video(video)?;
        writer.video = track;
        writer.video_encoder = name;
        if let Some(audio) = audio {
            writer.audio = Some(writer.open_audio(audio)?);
        }
        // SAFETY: the format context is valid; `pb` is null until opened here
        // and closed in `drop`; the header is written once, after every
        // stream was added.
        unsafe {
            let r = ff::avio_open(&mut (*writer.format).pb, c_path.as_ptr(), ff::AVIO_FLAG_WRITE);
            if r < 0 {
                return Err(failed("Opening the file for writing", r));
            }
            let r = ff::avformat_write_header(writer.format, ptr::null_mut());
            if r < 0 {
                return Err(failed("Starting the file", r));
            }
        }
        Ok(writer)
    }

    /// Which encoder makes the picture stream (FFmpeg's name for it).
    pub fn video_encoder(&self) -> &str {
        &self.video_encoder
    }

    /// Whether the picture is encoded on a graphics card.
    pub fn on_graphics_card(&self) -> bool {
        self.video_encoder != SOFTWARE_ENCODER
    }

    /// Whether the container wants stream headers up front.
    fn global_header(&self) -> bool {
        // SAFETY: the format context and its static output format are valid.
        unsafe { (*(*self.format).oformat).flags & ff::AVFMT_GLOBALHEADER != 0 }
    }

    /// Opens the first picture encoder that works and adds its stream.
    fn open_video(&mut self, settings: &VideoSettings) -> diag::Result<(Track, String)> {
        let mut last = cannot("This build has no H.264 encoder.");
        for name in VIDEO_ENCODERS {
            if settings.software && name != SOFTWARE_ENCODER {
                continue;
            }
            let c_name = CString::new(name).map_err(|_| cannot("An encoder's name is not text."))?;
            // SAFETY: a valid C string; the result is null or FFmpeg's static codec.
            let codec = unsafe { ff::avcodec_find_encoder_by_name(c_name.as_ptr()) };
            if codec.is_null() {
                continue;
            }
            match self.open_video_with(codec, name, settings) {
                Ok(track) => return Ok((track, name.to_owned())),
                Err(err) => {
                    tracing::info!("the encoder {name} did not open: {}", err.report());
                    last = err;
                }
            }
        }
        Err(last)
    }

    fn open_video_with(
        &mut self,
        codec: *const ff::AVCodec,
        name: &str,
        settings: &VideoSettings,
    ) -> diag::Result<Track> {
        let global_header = self.global_header();
        // SAFETY: `codec` is a valid encoder. The context is allocated here,
        // filled with plain values before it is opened, and freed again on
        // every early return; the option strings are valid C strings; the
        // frame is allocated with the context's own size and format.
        unsafe {
            let mut ctx = ff::avcodec_alloc_context3(codec);
            if ctx.is_null() {
                return Err(cannot("There is no memory for the encoder."));
            }
            let c = &mut *ctx;
            c.width = settings.width as i32;
            c.height = settings.height as i32;
            c.pix_fmt = ff::AVPixelFormat::AV_PIX_FMT_NV12;
            c.time_base = ff::AVRational {
                num: settings.rate.1 as i32,
                den: settings.rate.0 as i32,
            };
            c.framerate = ff::AVRational {
                num: settings.rate.0 as i32,
                den: settings.rate.1 as i32,
            };
            let bits = settings.bit_rate.min(i64::MAX as u64) as i64;
            c.bit_rate = bits;
            c.rc_max_rate = bits.saturating_mul(2);
            c.rc_buffer_size = bits.saturating_mul(2).min(i64::from(i32::MAX)) as i32;
            // A key frame every two seconds: quick to jump around in.
            let frames_a_second = (f64::from(settings.rate.0) / f64::from(settings.rate.1)).round() as i32;
            c.gop_size = (frames_a_second * 2).max(1);
            // What the compositor hands over: BT.709 in the limited range.
            c.color_range = ff::AVColorRange::AVCOL_RANGE_MPEG;
            c.color_primaries = ff::AVColorPrimaries::AVCOL_PRI_BT709;
            c.color_trc = ff::AVColorTransferCharacteristic::AVCOL_TRC_BT709;
            c.colorspace = ff::AVColorSpace::AVCOL_SPC_BT709;
            if global_header {
                c.flags |= ff::AV_CODEC_FLAG_GLOBAL_HEADER as i32;
            }
            if name == SOFTWARE_ENCODER {
                // Quick enough to keep up, still efficient.
                ff::av_opt_set(c.priv_data, c"preset".as_ptr(), c"veryfast".as_ptr(), 0);
            }
            let r = ff::avcodec_open2(ctx, codec, ptr::null_mut());
            if r < 0 {
                ff::avcodec_free_context(&mut ctx);
                return Err(failed(&format!("Opening the encoder {name}"), r));
            }
            let mut frame = ff::av_frame_alloc();
            if frame.is_null() {
                ff::avcodec_free_context(&mut ctx);
                return Err(cannot("There is no memory for a picture."));
            }
            (*frame).format = ff::AVPixelFormat::AV_PIX_FMT_NV12 as i32;
            (*frame).width = c.width;
            (*frame).height = c.height;
            let r = ff::av_frame_get_buffer(frame, 0);
            if r < 0 {
                ff::av_frame_free(&mut frame);
                ff::avcodec_free_context(&mut ctx);
                return Err(failed("Making room for a picture", r));
            }
            match self.add_stream(ctx) {
                Ok(stream) => Ok(Track {
                    codec: ctx,
                    stream,
                    frame,
                    next: 0,
                }),
                Err(err) => {
                    ff::av_frame_free(&mut frame);
                    ff::avcodec_free_context(&mut ctx);
                    Err(err)
                }
            }
        }
    }

    fn open_audio(&mut self, settings: &AudioSettings) -> diag::Result<Track> {
        let global_header = self.global_header();
        // SAFETY: as for the picture encoder: allocated here, filled with
        // plain values, opened, and freed on every early return. The frame
        // is given the size the opened encoder asks for.
        unsafe {
            let codec = ff::avcodec_find_encoder(ff::AVCodecID::AV_CODEC_ID_AAC);
            if codec.is_null() {
                return Err(cannot("This build has no AAC encoder."));
            }
            let mut ctx = ff::avcodec_alloc_context3(codec);
            if ctx.is_null() {
                return Err(cannot("There is no memory for the encoder."));
            }
            let c = &mut *ctx;
            c.sample_fmt = ff::AVSampleFormat::AV_SAMPLE_FMT_FLTP;
            c.sample_rate = settings.sample_rate as i32;
            ff::av_channel_layout_default(&mut c.ch_layout, CHANNELS as i32);
            c.bit_rate = settings.bit_rate.min(i64::MAX as u64) as i64;
            c.time_base = ff::AVRational {
                num: 1,
                den: settings.sample_rate as i32,
            };
            if global_header {
                c.flags |= ff::AV_CODEC_FLAG_GLOBAL_HEADER as i32;
            }
            let r = ff::avcodec_open2(ctx, codec, ptr::null_mut());
            if r < 0 {
                ff::avcodec_free_context(&mut ctx);
                return Err(failed("Opening the sound encoder", r));
            }
            let mut frame = ff::av_frame_alloc();
            if frame.is_null() {
                ff::avcodec_free_context(&mut ctx);
                return Err(cannot("There is no memory for the sound."));
            }
            (*frame).format = ff::AVSampleFormat::AV_SAMPLE_FMT_FLTP as i32;
            (*frame).sample_rate = c.sample_rate;
            (*frame).nb_samples = c.frame_size.max(1);
            let mut r = ff::av_channel_layout_copy(&mut (*frame).ch_layout, &c.ch_layout);
            if r >= 0 {
                r = ff::av_frame_get_buffer(frame, 0);
            }
            if r < 0 {
                ff::av_frame_free(&mut frame);
                ff::avcodec_free_context(&mut ctx);
                return Err(failed("Making room for the sound", r));
            }
            match self.add_stream(ctx) {
                Ok(stream) => Ok(Track {
                    codec: ctx,
                    stream,
                    frame,
                    next: 0,
                }),
                Err(err) => {
                    ff::av_frame_free(&mut frame);
                    ff::avcodec_free_context(&mut ctx);
                    Err(err)
                }
            }
        }
    }

    /// Adds a stream for an opened encoder to the file.
    ///
    /// # Safety
    /// `ctx` must be an opened encoder context.
    unsafe fn add_stream(&mut self, ctx: *mut ff::AVCodecContext) -> diag::Result<*mut ff::AVStream> {
        // SAFETY: the format context is valid and takes ownership of the
        // stream; the parameters are copied from an opened encoder.
        unsafe {
            let stream = ff::avformat_new_stream(self.format, ptr::null());
            if stream.is_null() {
                return Err(cannot("There is no memory for a stream."));
            }
            let r = ff::avcodec_parameters_from_context((*stream).codecpar, ctx);
            if r < 0 {
                return Err(failed("Describing a stream", r));
            }
            (*stream).time_base = (*ctx).time_base;
            Ok(stream)
        }
    }

    /// Adds one picture: `width × height` bytes of brightness, then
    /// `width × height / 2` bytes of interleaved colour, rows without padding.
    pub fn write_picture(&mut self, nv12: &[u8]) -> diag::Result<()> {
        let (w, h) = (self.size.0 as usize, self.size.1 as usize);
        if nv12.len() != w * h * 3 / 2 {
            return Err(cannot(format!(
                "A picture of {} bytes was handed over where {} were expected.",
                nv12.len(),
                w * h * 3 / 2
            )));
        }
        let (luma, chroma) = nv12.split_at(w * h);
        // SAFETY: the frame was allocated for this size as NV12, so plane 0
        // has `h` rows and plane 1 `h / 2` rows of at least `w` bytes each
        // (`linesize` may be larger: rows are copied one by one); it is made
        // writable first, since the encoder may still hold the last one.
        unsafe {
            let frame = self.video.frame;
            let r = ff::av_frame_make_writable(frame);
            if r < 0 {
                return Err(failed("Making room for a picture", r));
            }
            for (plane, rows, bytes) in [(0, h, luma), (1, h / 2, chroma)] {
                let stride = (*frame).linesize[plane] as usize;
                let start = (*frame).data[plane];
                for (row, line) in bytes.chunks_exact(w).take(rows).enumerate() {
                    ptr::copy_nonoverlapping(line.as_ptr(), start.add(row * stride), w);
                }
            }
            (*frame).pts = self.video.next;
        }
        self.video.next += 1;
        let (codec, stream, frame) = (self.video.codec, self.video.stream, self.video.frame);
        self.encode(codec, stream, frame)
    }

    /// Adds sound: interleaved stereo. It is encoded as full frames fill up;
    /// what is left over goes out with [`Writer::finish`].
    pub fn write_sound(&mut self, samples: &[f32]) -> diag::Result<()> {
        if self.audio.is_none() {
            return Ok(());
        }
        self.waiting.extend_from_slice(samples);
        self.encode_sound(false)
    }

    /// Encodes the waiting samples a frame at a time; with `all`, the last
    /// partial frame too.
    fn encode_sound(&mut self, all: bool) -> diag::Result<()> {
        let Some(audio) = &self.audio else {
            return Ok(());
        };
        let (codec, stream, frame) = (audio.codec, audio.stream, audio.frame);
        // SAFETY: the context is an opened encoder.
        let size = unsafe { (*codec).frame_size.max(1) } as usize;
        let mut done = 0;
        loop {
            let left = (self.waiting.len() - done) / CHANNELS;
            let count = if left >= size {
                size
            } else if all && left > 0 {
                left
            } else {
                break;
            };
            let chunk = &self.waiting[done..done + count * CHANNELS];
            // SAFETY: the frame was allocated with `size` samples in two
            // float planes; `count <= size` samples are written to each, and
            // it is made writable first.
            unsafe {
                let r = ff::av_frame_make_writable(frame);
                if r < 0 {
                    return Err(failed("Making room for the sound", r));
                }
                (*frame).nb_samples = count as i32;
                for channel in 0..CHANNELS {
                    let plane = (*frame).data[channel].cast::<f32>();
                    for (i, pair) in chunk.chunks_exact(CHANNELS).enumerate() {
                        *plane.add(i) = pair[channel];
                    }
                }
            }
            let next = self.audio.as_ref().map_or(0, |a| a.next);
            // SAFETY: the frame is valid and ours.
            unsafe { (*frame).pts = next };
            if let Some(audio) = &mut self.audio {
                audio.next += count as i64;
            }
            done += count * CHANNELS;
            self.encode(codec, stream, frame)?;
        }
        self.waiting.drain(..done);
        Ok(())
    }

    /// Hands `frame` (null: no more) to an encoder and writes what comes out.
    fn encode(
        &mut self,
        codec: *mut ff::AVCodecContext,
        stream: *mut ff::AVStream,
        frame: *const ff::AVFrame,
    ) -> diag::Result<()> {
        // SAFETY: the context is an opened encoder, the frame is null or a
        // filled frame of its format, the packet is ours and unreferenced
        // after each use (the muxer takes its contents), and the stream
        // belongs to the open format context.
        unsafe {
            let r = ff::avcodec_send_frame(codec, frame);
            if r < 0 && r != ff::AVERROR_EOF {
                return Err(failed("Encoding", r));
            }
            loop {
                let r = ff::avcodec_receive_packet(codec, self.packet);
                if r == ff::AVERROR(ff::EAGAIN) || r == ff::AVERROR_EOF {
                    return Ok(());
                }
                if r < 0 {
                    return Err(failed("Encoding", r));
                }
                ff::av_packet_rescale_ts(self.packet, (*codec).time_base, (*stream).time_base);
                (*self.packet).stream_index = (*stream).index;
                let r = ff::av_interleaved_write_frame(self.format, self.packet);
                ff::av_packet_unref(self.packet);
                if r < 0 {
                    return Err(failed("Writing the file", r));
                }
            }
        }
    }

    /// Encodes what is still waiting and completes the file. Without this
    /// the file is cut short and may not play.
    pub fn finish(mut self) -> diag::Result<()> {
        self.encode_sound(true)?;
        let (codec, stream) = (self.video.codec, self.video.stream);
        self.encode(codec, stream, ptr::null())?;
        if let Some((codec, stream)) = self.audio.as_ref().map(|a| (a.codec, a.stream)) {
            self.encode(codec, stream, ptr::null())?;
        }
        // SAFETY: the header was written, so the trailer may be; the file is
        // closed here and not again in `drop` (`avio_closep` nulls `pb`).
        unsafe {
            let r = ff::av_write_trailer(self.format);
            if r < 0 {
                return Err(failed("Completing the file", r));
            }
            let r = ff::avio_closep(&mut (*self.format).pb);
            if r < 0 {
                return Err(failed("Closing the file", r));
            }
        }
        Ok(())
    }
}

impl Track {
    fn empty() -> Track {
        Track {
            codec: ptr::null_mut(),
            stream: ptr::null_mut(),
            frame: ptr::null_mut(),
            next: 0,
        }
    }

    fn free(&mut self) {
        // SAFETY: each pointer is null or was allocated by the matching
        // FFmpeg call and is freed once (the calls null it). The stream
        // belongs to the format context.
        unsafe {
            ff::av_frame_free(&mut self.frame);
            ff::avcodec_free_context(&mut self.codec);
        }
    }
}

impl Drop for Writer {
    fn drop(&mut self) {
        self.video.free();
        if let Some(audio) = &mut self.audio {
            audio.free();
        }
        // SAFETY: each pointer is null or valid and freed once; the file is
        // closed if `finish` didn't get to it.
        unsafe {
            ff::av_packet_free(&mut self.packet);
            if !self.format.is_null() {
                if !(*self.format).pb.is_null() {
                    ff::avio_closep(&mut (*self.format).pb);
                }
                ff::avformat_free_context(self.format);
                self.format = ptr::null_mut();
            }
        }
    }
}

/// The names of the H.264 encoders this build has, in the order tried.
pub fn video_encoders_present() -> Vec<&'static str> {
    VIDEO_ENCODERS
        .into_iter()
        .filter(|name| {
            let Ok(c_name) = CString::new(*name) else {
                return false;
            };
            // SAFETY: a valid C string; only compared with null.
            !unsafe { ff::avcodec_find_encoder_by_name(c_name.as_ptr()) }.is_null()
        })
        .collect()
}

/// FFmpeg's long name for an encoder, for the log.
pub fn encoder_description(name: &str) -> Option<String> {
    let c_name = CString::new(name).ok()?;
    // SAFETY: a valid C string; the codec and its long name are FFmpeg's
    // static data when not null.
    unsafe {
        let codec = ff::avcodec_find_encoder_by_name(c_name.as_ptr());
        if codec.is_null() || (*codec).long_name.is_null() {
            return None;
        }
        Some(CStr::from_ptr((*codec).long_name).to_string_lossy().into_owned())
    }
}
