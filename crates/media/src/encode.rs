//! Writing a file: pictures and sound in, a video or sound file out.
//!
//! [`Writer`] takes pictures as NV12 (brightness at full size, then the two
//! colour samples interleaved at half size: what the compositor reads back)
//! and sound as interleaved stereo floats, encodes both and writes them
//! interleaved. What it writes is chosen from [`formats`]: the container
//! ([`Container`]: MP4, MKV, or sound alone as WAV, MP3 or OGG), the picture
//! codec ([`VideoCodec`]: H.264, H.265, AV1) and the sound codec
//! ([`AudioCodec`]); [`Container::allows`] says which go together.
//!
//! A picture codec's encoders are tried in order ([`VideoCodec::encoders`]):
//! the graphics card's own first, then the system's, then the processor's.
//! The first that opens makes the pictures. [`probe_video`] does the same
//! trial without a file, for saying what this machine can encode.

use crate::ffi::error_text;
use diag::AppError;
use ffmpeg_sys_next as ff;
use std::ffi::{CStr, CString, c_int, c_void};
use std::path::Path;
use std::ptr;

pub mod formats;
mod sound;

pub use formats::{AudioCodec, Container, Encoder, Place, VideoCodec, combinations};
use sound::Layout;

const CHANNELS: usize = 2;
/// Samples a sound frame holds when the encoder takes any number (plain
/// samples for WAV).
const ANY_FRAME_SIZE: usize = 1024;

/// What the picture stream is like.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VideoSettings {
    pub codec: VideoCodec,
    /// Both even: colour is stored at half size.
    pub width: u32,
    pub height: u32,
    /// Frames a second, as a fraction (60000/1001 for 59.94).
    pub rate: (u32, u32),
    /// Bits a second to aim for.
    pub bit_rate: u64,
    /// Encode on the processor even when a graphics card could.
    pub software: bool,
}

/// What the sound stream is like: stereo at this rate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AudioSettings {
    pub codec: AudioCodec,
    pub sample_rate: u32,
    /// Bits a second; not used for lossless codecs.
    pub bit_rate: u64,
}

fn cannot(why: impl Into<String>) -> AppError {
    AppError::new("The file can't be exported")
        .why(why)
        .fix("Check that the folder can be written to and has room, then export again. If it keeps failing, choose the processor as the encoder.")
}

fn failed(what: &str, code: i32) -> AppError {
    cannot(format!("{what} failed: {}.", error_text(code)))
}

fn no_encoder(what: &str) -> AppError {
    AppError::new("The file can't be exported")
        .why(format!("This build of the app has no {what} encoder."))
        .fix("Choose another format.")
}

/// One stream's encoder and where its packets go.
struct Track {
    codec: *mut ff::AVCodecContext,
    stream: *mut ff::AVStream,
    /// The frame filled in memory.
    frame: *mut ff::AVFrame,
    /// For an encoder that takes frames on its video device: the frame the
    /// picture is uploaded into before it is encoded (else null).
    upload: *mut ff::AVFrame,
    /// Frames (pictures or samples) handed to the encoder so far.
    next: i64,
    /// Pictures only: the encoder takes the two colour samples in planes of
    /// their own (4:2:0 planar) rather than interleaved.
    planar_colour: bool,
    /// Sound only: how the samples are laid out, how many bits whole
    /// numbers carry, and how many samples a frame holds.
    layout: Layout,
    bits: u32,
    frame_samples: usize,
}

/// A file being written.
pub struct Writer {
    format: *mut ff::AVFormatContext,
    packet: *mut ff::AVPacket,
    container: Container,
    video: Option<Track>,
    video_encoder: Option<Encoder>,
    audio: Option<Track>,
    audio_encoder: Option<&'static str>,
    size: (u32, u32),
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
            "Writer({} {}×{} with {:?} and {:?})",
            self.container.label(),
            self.size.0,
            self.size.1,
            self.video_encoder.map(|e| e.name),
            self.audio_encoder
        )
    }
}

impl Writer {
    /// Starts a file of kind `container` at `path` (replacing one that is
    /// there), with pictures when `video` is given (always for MP4 and MKV,
    /// never for the others) and sound when `audio` is.
    pub fn create(
        path: &Path,
        container: Container,
        video: Option<&VideoSettings>,
        audio: Option<&AudioSettings>,
    ) -> diag::Result<Writer> {
        check_combination(container, video, audio)?;
        if let Some(video) = video {
            check_picture(video)?;
        }
        let c_path = path
            .to_str()
            .and_then(|text| CString::new(text).ok())
            .ok_or_else(|| cannot("The file's name has characters that can't be passed on."))?;
        let mut format = ptr::null_mut();
        // SAFETY: `format` starts null so FFmpeg allocates it; the muxer's
        // name and the path are valid C strings during the call.
        let r = unsafe {
            ff::avformat_alloc_output_context2(
                &mut format,
                ptr::null(),
                container.muxer().as_ptr(),
                c_path.as_ptr(),
            )
        };
        if r < 0 || format.is_null() {
            return Err(failed("Starting the file", r));
        }
        // From here on `writer` owns everything made, and frees it when dropped.
        let mut writer = Writer {
            format,
            packet: ptr::null_mut(),
            container,
            video: None,
            video_encoder: None,
            audio: None,
            audio_encoder: None,
            size: video.map_or((0, 0), |v| (v.width, v.height)),
            waiting: Vec::new(),
        };
        // SAFETY: allocates a packet, checked for null below.
        writer.packet = unsafe { ff::av_packet_alloc() };
        if writer.packet.is_null() {
            return Err(cannot("There is no memory for it."));
        }
        let global_header = writer.global_header();
        if let Some(video) = video {
            let (opened, encoder) = open_video(video, global_header)?;
            // SAFETY: an opened encoder context, owned by the track from here.
            let stream = unsafe { writer.add_stream(opened.codec) };
            writer.video = Some(opened);
            writer.video_encoder = Some(encoder);
            if let Some(track) = &mut writer.video {
                track.stream = stream?;
            }
        }
        if let Some(audio) = audio {
            let (opened, name) = open_audio(audio, global_header)?;
            // SAFETY: as above.
            let stream = unsafe { writer.add_stream(opened.codec) };
            writer.audio = Some(opened);
            writer.audio_encoder = Some(name);
            if let Some(track) = &mut writer.audio {
                track.stream = stream?;
            }
        }
        // SAFETY: the format context is valid; `pb` is null until opened here
        // and closed in `drop`; the header is written once, after every
        // stream was added; the options dictionary is freed after the call.
        unsafe {
            let r = ff::avio_open(&mut (*writer.format).pb, c_path.as_ptr(), ff::AVIO_FLAG_WRITE);
            if r < 0 {
                return Err(failed("Opening the file for writing", r));
            }
            let mut options: *mut ff::AVDictionary = ptr::null_mut();
            if container == Container::Mp4 {
                // The index goes at the front when the file is complete, so
                // it starts playing before it has all downloaded.
                ff::av_dict_set(&mut options, c"movflags".as_ptr(), c"+faststart".as_ptr(), 0);
            }
            let r = ff::avformat_write_header(writer.format, &mut options);
            ff::av_dict_free(&mut options);
            if r < 0 {
                return Err(failed("Starting the file", r));
            }
        }
        Ok(writer)
    }

    /// Which encoder makes the picture stream (FFmpeg's name for it), when
    /// there is one.
    pub fn video_encoder(&self) -> Option<&'static str> {
        self.video_encoder.map(|e| e.name)
    }

    /// Which encoder makes the sound (FFmpeg's name for it), when there is sound.
    pub fn audio_encoder(&self) -> Option<&'static str> {
        self.audio_encoder
    }

    /// Whether the picture is encoded on a graphics card (or the system's
    /// encoder, which uses one when it can).
    pub fn on_graphics_card(&self) -> bool {
        self.video_encoder.is_some_and(|e| e.place != Place::Processor)
    }

    /// Whether the container wants stream headers up front.
    fn global_header(&self) -> bool {
        // SAFETY: the format context and its static output format are valid.
        unsafe { (*(*self.format).oformat).flags & ff::AVFMT_GLOBALHEADER != 0 }
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
            if self.container == Container::Mp4 && (*ctx).codec_id == ff::AVCodecID::AV_CODEC_ID_HEVC {
                // Tagged so every player that takes H.265 in MP4 knows it.
                (*(*stream).codecpar).codec_tag = u32::from_le_bytes(*b"hvc1");
            }
            Ok(stream)
        }
    }

    /// Adds one picture: `width × height` bytes of brightness, then
    /// `width × height / 2` bytes of interleaved colour, rows without padding.
    pub fn write_picture(&mut self, nv12: &[u8]) -> diag::Result<()> {
        let (w, h) = (self.size.0 as usize, self.size.1 as usize);
        let Some(video) = &mut self.video else {
            return Err(cannot(format!(
                "A picture was handed over for a {} file, which holds sound only.",
                self.container.label()
            )));
        };
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
        // writable first, since the encoder may still hold the last one. An
        // upload frame comes from the encoder's own pool and is filled from
        // the frame in memory.
        let send = unsafe {
            let frame = video.frame;
            let r = ff::av_frame_make_writable(frame);
            if r < 0 {
                return Err(failed("Making room for a picture", r));
            }
            let rows = |plane: usize, bytes: &[u8], count: usize| {
                let stride = (*frame).linesize[plane] as usize;
                let start = (*frame).data[plane];
                for (row, line) in bytes.chunks_exact(w).take(count).enumerate() {
                    ptr::copy_nonoverlapping(line.as_ptr(), start.add(row * stride), w);
                }
            };
            rows(0, luma, h);
            if video.planar_colour {
                // Planes 1 and 2 have `h / 2` rows of at least `w / 2` bytes.
                for (row, line) in chroma.chunks_exact(w).take(h / 2).enumerate() {
                    let u = (*frame).data[1].add(row * (*frame).linesize[1] as usize);
                    let v = (*frame).data[2].add(row * (*frame).linesize[2] as usize);
                    for (i, pair) in line.chunks_exact(2).enumerate() {
                        *u.add(i) = pair[0];
                        *v.add(i) = pair[1];
                    }
                }
            } else {
                rows(1, chroma, h / 2);
            }
            (*frame).pts = video.next;
            // Each picture lasts one frame: the container's length and the
            // last picture's place depend on it.
            (*frame).duration = 1;
            if video.upload.is_null() {
                frame
            } else {
                ff::av_frame_unref(video.upload);
                let r = ff::av_hwframe_get_buffer((*video.codec).hw_frames_ctx, video.upload, 0);
                if r < 0 {
                    return Err(failed("Making room for a picture on the graphics card", r));
                }
                let r = ff::av_hwframe_transfer_data(video.upload, frame, 0);
                if r < 0 {
                    return Err(failed("Handing a picture to the graphics card", r));
                }
                (*video.upload).pts = video.next;
                (*video.upload).duration = 1;
                video.upload
            }
        };
        video.next += 1;
        let (codec, stream) = (video.codec, video.stream);
        self.encode(codec, stream, send)
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
        let (layout, bits, size) = (audio.layout, audio.bits, audio.frame_samples);
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
            let next = self.audio.as_ref().map_or(0, |a| a.next);
            // SAFETY: the frame was allocated with `size` samples in the
            // encoder's layout (two planes, or one holding both channels);
            // `count <= size` samples are written, within each plane's
            // bytes, and it is made writable first.
            unsafe {
                let r = ff::av_frame_make_writable(frame);
                if r < 0 {
                    return Err(failed("Making room for the sound", r));
                }
                (*frame).nb_samples = count as i32;
                (*frame).pts = next;
                (*frame).duration = count as i64;
                let per_plane = if layout.planar() { 1 } else { CHANNELS };
                let bytes = size * per_plane * layout.bytes_per_sample();
                let planes = if layout.planar() { CHANNELS } else { 1 };
                let mut slices: Vec<&mut [u8]> = (0..planes)
                    .map(|p| std::slice::from_raw_parts_mut((*frame).data[p], bytes))
                    .collect();
                sound::write(chunk, CHANNELS, layout, bits, &mut slices);
            }
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
                if (*codec).codec_type == ff::AVMediaType::AVMEDIA_TYPE_VIDEO && (*self.packet).duration <= 0
                {
                    // Every picture lasts one frame. Some encoders leave the
                    // last packets without a length, and MP4 then ends the
                    // picture early, leaving the last frame out.
                    (*self.packet).duration = 1;
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
        for (codec, stream) in [&self.video, &self.audio]
            .into_iter()
            .flatten()
            .map(|t| (t.codec, t.stream))
            .collect::<Vec<_>>()
        {
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

/// Refuses, with the reason, a picture and sound the container can't hold.
fn check_combination(
    container: Container,
    video: Option<&VideoSettings>,
    audio: Option<&AudioSettings>,
) -> diag::Result<()> {
    let wrong = |why: String| {
        AppError::new("The file can't be exported")
            .why(why)
            .fix("Choose a format from the list offered.")
    };
    match (container.has_video(), video) {
        (true, None) => {
            return Err(wrong(format!(
                "A {} file is a video, and no picture settings were given.",
                container.label()
            )));
        }
        (false, Some(_)) => {
            return Err(wrong(format!(
                "A {} file holds sound only, not a picture.",
                container.label()
            )));
        }
        (true, Some(video)) if !container.video_codecs().contains(&video.codec) => {
            return Err(wrong(format!(
                "A {} file can't hold {} pictures.",
                container.label(),
                video.codec.label()
            )));
        }
        _ => {}
    }
    match audio {
        Some(audio) if !container.audio_codecs().contains(&audio.codec) => Err(wrong(format!(
            "A {} file can't hold {} sound.",
            container.label(),
            audio.codec.label()
        ))),
        None if !container.has_video() => Err(wrong(format!(
            "A {} file holds sound, and no sound settings were given.",
            container.label()
        ))),
        _ => Ok(()),
    }
}

fn check_picture(video: &VideoSettings) -> diag::Result<()> {
    if video.width == 0
        || video.height == 0
        || !video.width.is_multiple_of(2)
        || !video.height.is_multiple_of(2)
    {
        return Err(cannot(format!(
            "A picture of {}×{} can't be encoded: both sides must be even.",
            video.width, video.height
        )));
    }
    if video.rate.0 == 0 || video.rate.1 == 0 {
        return Err(cannot("The frame rate is not a number."));
    }
    Ok(())
}

/// The encoder FFmpeg knows by `name`, if this build has it.
fn find_encoder(name: &str) -> Option<*const ff::AVCodec> {
    let c_name = CString::new(name).ok()?;
    // SAFETY: a valid C string; the result is null or FFmpeg's static codec.
    let codec = unsafe { ff::avcodec_find_encoder_by_name(c_name.as_ptr()) };
    (!codec.is_null()).then_some(codec)
}

/// Opens the first of the codec's encoders that works (only the
/// processor's when `software`), as a track not yet in a file.
fn open_video(settings: &VideoSettings, global_header: bool) -> diag::Result<(Track, Encoder)> {
    let mut last = None;
    for encoder in settings.codec.encoders() {
        if settings.software && encoder.place != Place::Processor {
            continue;
        }
        let Some(codec) = find_encoder(encoder.name) else {
            continue;
        };
        match open_video_with(codec, encoder, settings, global_header) {
            Ok(track) => return Ok((track, *encoder)),
            Err(err) => {
                tracing::info!("the encoder {} did not open: {}", encoder.name, err.report());
                last = Some(err);
            }
        }
    }
    Err(last.unwrap_or_else(|| no_encoder(settings.codec.label())))
}

fn open_video_with(
    codec: *const ff::AVCodec,
    encoder: &Encoder,
    settings: &VideoSettings,
    global_header: bool,
) -> diag::Result<Track> {
    let name = encoder.name;
    // Interleaved colour (NV12) as read back when the encoder takes it, else
    // in planes (the processor's H.265 and AV1 encoders take only those).
    // SAFETY: `codec` is a valid encoder.
    let formats = unsafe { pixel_formats(codec) };
    let planar_colour = !encoder.needs_upload
        && !formats.is_empty()
        && !formats.contains(&ff::AVPixelFormat::AV_PIX_FMT_NV12)
        && formats.contains(&ff::AVPixelFormat::AV_PIX_FMT_YUV420P);
    let pix_fmt = if planar_colour {
        ff::AVPixelFormat::AV_PIX_FMT_YUV420P
    } else {
        ff::AVPixelFormat::AV_PIX_FMT_NV12
    };
    // SAFETY: `codec` is a valid encoder. The context is allocated here,
    // filled with plain values before it is opened, and freed again on
    // every early return (with the device frames it was given); the option
    // strings are valid C strings; the frame is allocated with the
    // context's own size and format.
    unsafe {
        let mut ctx = ff::avcodec_alloc_context3(codec);
        if ctx.is_null() {
            return Err(cannot("There is no memory for the encoder."));
        }
        let c = &mut *ctx;
        c.width = settings.width as i32;
        c.height = settings.height as i32;
        c.pix_fmt = pix_fmt;
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
        match name {
            // Quick enough to keep up, still efficient.
            "libx264" => {
                ff::av_opt_set(c.priv_data, c"preset".as_ptr(), c"veryfast".as_ptr(), 0);
            }
            "libx265" => {
                ff::av_opt_set(c.priv_data, c"preset".as_ptr(), c"veryfast".as_ptr(), 0);
                ff::av_opt_set(
                    c.priv_data,
                    c"x265-params".as_ptr(),
                    c"log-level=error".as_ptr(),
                    0,
                );
            }
            "libsvtav1" => {
                // 0 is the slowest and smallest, 13 the quickest.
                ff::av_opt_set(c.priv_data, c"preset".as_ptr(), c"8".as_ptr(), 0);
                // Its variable bit rate takes no ceiling.
                c.rc_max_rate = 0;
                c.rc_buffer_size = 0;
            }
            _ => {}
        }
        if encoder.needs_upload
            && let Err(err) = attach_device_frames(ctx, settings)
        {
            ff::avcodec_free_context(&mut ctx);
            return Err(err);
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
        (*frame).format = pix_fmt as i32;
        (*frame).width = settings.width as i32;
        (*frame).height = settings.height as i32;
        let r = ff::av_frame_get_buffer(frame, 0);
        let upload = if encoder.needs_upload {
            ff::av_frame_alloc()
        } else {
            ptr::null_mut()
        };
        if r < 0 || (encoder.needs_upload && upload.is_null()) {
            ff::av_frame_free(&mut frame);
            let mut upload = upload;
            ff::av_frame_free(&mut upload);
            ff::avcodec_free_context(&mut ctx);
            return Err(failed("Making room for a picture", r.min(-1)));
        }
        Ok(Track {
            codec: ctx,
            stream: ptr::null_mut(),
            frame,
            upload,
            next: 0,
            planar_colour,
            layout: Layout::Float,
            bits: 0,
            frame_samples: 0,
        })
    }
}

/// Gives an encoder that takes frames on its video device (VA-API) a pool
/// of such frames, on the system's first video device.
///
/// # Safety
/// `ctx` must be an encoder context not yet opened.
unsafe fn attach_device_frames(ctx: *mut ff::AVCodecContext, settings: &VideoSettings) -> diag::Result<()> {
    // SAFETY: the device and the pool are FFmpeg's reference-counted
    // buffers; each reference made here is released here, and the context
    // keeps its own reference to the pool (freed with the context).
    unsafe {
        let mut device = ptr::null_mut();
        let r = ff::av_hwdevice_ctx_create(
            &mut device,
            ff::AVHWDeviceType::AV_HWDEVICE_TYPE_VAAPI,
            ptr::null(),
            ptr::null_mut(),
            0,
        );
        if r < 0 {
            return Err(failed("Opening the video device", r));
        }
        let mut pool = ff::av_hwframe_ctx_alloc(device);
        ff::av_buffer_unref(&mut device);
        if pool.is_null() {
            return Err(cannot("There is no memory for pictures on the graphics card."));
        }
        let frames = (*pool).data as *mut ff::AVHWFramesContext;
        (*frames).format = ff::AVPixelFormat::AV_PIX_FMT_VAAPI;
        (*frames).sw_format = ff::AVPixelFormat::AV_PIX_FMT_NV12;
        (*frames).width = settings.width as c_int;
        (*frames).height = settings.height as c_int;
        (*frames).initial_pool_size = 16;
        let r = ff::av_hwframe_ctx_init(pool);
        if r < 0 {
            ff::av_buffer_unref(&mut pool);
            return Err(failed("Making pictures on the graphics card", r));
        }
        (*ctx).pix_fmt = ff::AVPixelFormat::AV_PIX_FMT_VAAPI;
        (*ctx).hw_frames_ctx = ff::av_buffer_ref(pool);
        ff::av_buffer_unref(&mut pool);
        if (*ctx).hw_frames_ctx.is_null() {
            return Err(cannot("There is no memory for pictures on the graphics card."));
        }
        Ok(())
    }
}

/// The pixel formats an encoder takes (empty: any, or not said).
///
/// # Safety
/// `codec` must be a valid encoder.
unsafe fn pixel_formats(codec: *const ff::AVCodec) -> Vec<ff::AVPixelFormat> {
    // SAFETY: FFmpeg hands back a pointer to its own static list and its
    // length (or null for "any"); it is only read, within that length.
    unsafe {
        let mut list: *const c_void = ptr::null();
        let mut count: c_int = 0;
        let r = ff::avcodec_get_supported_config(
            ptr::null(),
            codec,
            ff::AVCodecConfig::AV_CODEC_CONFIG_PIX_FORMAT,
            0,
            &mut list,
            &mut count,
        );
        if r < 0 || list.is_null() {
            return Vec::new();
        }
        std::slice::from_raw_parts(list.cast::<ff::AVPixelFormat>(), count.max(0) as usize).to_vec()
    }
}

/// The sample layouts and rates an encoder takes (empty: any).
///
/// # Safety
/// `codec` must be a valid encoder.
unsafe fn supported(codec: *const ff::AVCodec) -> (Vec<Layout>, Vec<u32>) {
    let mut layouts = Vec::new();
    let mut rates = Vec::new();
    // SAFETY: FFmpeg hands back a pointer to its own static list and its
    // length (or null for "any"); it is only read, within that length.
    unsafe {
        let mut list: *const c_void = ptr::null();
        let mut count: c_int = 0;
        let r = ff::avcodec_get_supported_config(
            ptr::null(),
            codec,
            ff::AVCodecConfig::AV_CODEC_CONFIG_SAMPLE_FORMAT,
            0,
            &mut list,
            &mut count,
        );
        if r >= 0 && !list.is_null() {
            let formats =
                std::slice::from_raw_parts(list.cast::<ff::AVSampleFormat>(), count.max(0) as usize);
            for format in formats {
                use ff::AVSampleFormat as F;
                layouts.push(match format {
                    F::AV_SAMPLE_FMT_FLTP => Layout::FloatPlanes,
                    F::AV_SAMPLE_FMT_FLT => Layout::Float,
                    F::AV_SAMPLE_FMT_S32P => Layout::WholePlanes,
                    F::AV_SAMPLE_FMT_S32 => Layout::Whole,
                    F::AV_SAMPLE_FMT_S16P => Layout::ShortPlanes,
                    F::AV_SAMPLE_FMT_S16 => Layout::Short,
                    _ => continue,
                });
            }
        }
        let mut list: *const c_void = ptr::null();
        let mut count: c_int = 0;
        let r = ff::avcodec_get_supported_config(
            ptr::null(),
            codec,
            ff::AVCodecConfig::AV_CODEC_CONFIG_SAMPLE_RATE,
            0,
            &mut list,
            &mut count,
        );
        if r >= 0 && !list.is_null() {
            let list = std::slice::from_raw_parts(list.cast::<c_int>(), count.max(0) as usize);
            rates = list.iter().map(|&r| r.max(0) as u32).collect();
        }
    }
    (layouts, rates)
}

fn sample_format(layout: Layout) -> ff::AVSampleFormat {
    use ff::AVSampleFormat as F;
    match layout {
        Layout::FloatPlanes => F::AV_SAMPLE_FMT_FLTP,
        Layout::Float => F::AV_SAMPLE_FMT_FLT,
        Layout::WholePlanes => F::AV_SAMPLE_FMT_S32P,
        Layout::Whole => F::AV_SAMPLE_FMT_S32,
        Layout::ShortPlanes => F::AV_SAMPLE_FMT_S16P,
        Layout::Short => F::AV_SAMPLE_FMT_S16,
    }
}

/// Opens the codec's first encoder this build has, as a track not yet in a file.
fn open_audio(settings: &AudioSettings, global_header: bool) -> diag::Result<(Track, &'static str)> {
    let (name, codec) = settings
        .codec
        .encoders()
        .iter()
        .find_map(|&name| find_encoder(name).map(|codec| (name, codec)))
        .ok_or_else(|| no_encoder(settings.codec.label()))?;
    // SAFETY: `codec` is a valid encoder.
    let (layouts, rates) = unsafe { supported(codec) };
    let layout = if layouts.is_empty() {
        Some(Layout::FloatPlanes)
    } else {
        Layout::PREFERRED.into_iter().find(|l| layouts.contains(l))
    }
    .ok_or_else(|| {
        cannot(format!(
            "The sound encoder {name} takes no sample layout this app writes."
        ))
    })?;
    if !rates.is_empty() && !rates.contains(&settings.sample_rate) {
        return Err(cannot(format!(
            "{} sound can't be made at {} samples a second.",
            settings.codec.label(),
            settings.sample_rate
        ))
        .fix("Change the timeline's sample rate to 48 kHz, or choose another sound format."));
    }
    // 24 bits for lossless sound kept in whole numbers; the full width otherwise.
    let bits = if settings.codec.lossless() { 24 } else { 32 };
    // SAFETY: as for the picture encoder: allocated here, filled with plain
    // values, opened, and freed on every early return. The frame is given
    // the size the opened encoder asks for (or a fixed size when it takes
    // any).
    unsafe {
        let mut ctx = ff::avcodec_alloc_context3(codec);
        if ctx.is_null() {
            return Err(cannot("There is no memory for the encoder."));
        }
        let c = &mut *ctx;
        c.sample_fmt = sample_format(layout);
        c.sample_rate = settings.sample_rate as i32;
        ff::av_channel_layout_default(&mut c.ch_layout, CHANNELS as i32);
        if !settings.codec.lossless() {
            c.bit_rate = settings.bit_rate.min(i64::MAX as u64) as i64;
        }
        if matches!(layout, Layout::Whole | Layout::WholePlanes) {
            c.bits_per_raw_sample = bits as i32;
        }
        c.time_base = ff::AVRational {
            num: 1,
            den: settings.sample_rate as i32,
        };
        if global_header {
            c.flags |= ff::AV_CODEC_FLAG_GLOBAL_HEADER as i32;
        }
        if name == "opus" {
            // FFmpeg's own Opus encoder is marked experimental.
            c.strict_std_compliance = ff::FF_COMPLIANCE_EXPERIMENTAL;
        }
        let r = ff::avcodec_open2(ctx, codec, ptr::null_mut());
        if r < 0 {
            ff::avcodec_free_context(&mut ctx);
            return Err(failed(&format!("Opening the sound encoder {name}"), r));
        }
        let variable = (*codec).capabilities & ff::AV_CODEC_CAP_VARIABLE_FRAME_SIZE as i32 != 0;
        let frame_samples = if c.frame_size <= 0 || variable {
            ANY_FRAME_SIZE
        } else {
            c.frame_size as usize
        };
        let mut frame = ff::av_frame_alloc();
        if frame.is_null() {
            ff::avcodec_free_context(&mut ctx);
            return Err(cannot("There is no memory for the sound."));
        }
        (*frame).format = c.sample_fmt as i32;
        (*frame).sample_rate = c.sample_rate;
        (*frame).nb_samples = frame_samples as i32;
        let mut r = ff::av_channel_layout_copy(&mut (*frame).ch_layout, &c.ch_layout);
        if r >= 0 {
            r = ff::av_frame_get_buffer(frame, 0);
        }
        if r < 0 {
            ff::av_frame_free(&mut frame);
            ff::avcodec_free_context(&mut ctx);
            return Err(failed("Making room for the sound", r));
        }
        Ok((
            Track {
                codec: ctx,
                stream: ptr::null_mut(),
                frame,
                upload: ptr::null_mut(),
                next: 0,
                planar_colour: false,
                layout,
                bits,
                frame_samples,
            },
            name,
        ))
    }
}

impl Track {
    fn free(&mut self) {
        // SAFETY: each pointer is null or was allocated by the matching
        // FFmpeg call and is freed once (the calls null it). The stream
        // belongs to the format context.
        unsafe {
            ff::av_frame_free(&mut self.frame);
            ff::av_frame_free(&mut self.upload);
            ff::avcodec_free_context(&mut self.codec);
        }
    }
}

impl Drop for Track {
    fn drop(&mut self) {
        self.free();
    }
}

impl Drop for Writer {
    fn drop(&mut self) {
        // The encoders go before the file.
        self.video = None;
        self.audio = None;
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

/// What this machine can encode a picture codec with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Probed {
    /// The first graphics card (or system) encoder that opened.
    pub graphics_card: Option<&'static str>,
    /// The processor's encoder, when this build has it.
    pub processor: Option<&'static str>,
}

impl Probed {
    /// Whether the codec can be encoded at all.
    pub fn any(&self) -> bool {
        self.graphics_card.is_some() || self.processor.is_some()
    }
}

/// Which encoders this machine has for `codec`: each graphics card encoder
/// this build has is opened briefly, at 1280×720, as an export would.
/// Takes up to a few hundred milliseconds with a graphics card (opening its
/// encoder is not free): call it on a worker, never the interface thread.
pub fn probe_video(codec: VideoCodec) -> Probed {
    let trial = VideoSettings {
        codec,
        width: 1280,
        height: 720,
        rate: (30, 1),
        bit_rate: 5_000_000,
        software: false,
    };
    let mut probed = Probed::default();
    for encoder in codec.encoders() {
        let Some(found) = find_encoder(encoder.name) else {
            continue;
        };
        if encoder.place == Place::Processor {
            probed.processor = Some(encoder.name);
        } else if probed.graphics_card.is_none() {
            match open_video_with(found, encoder, &trial, true) {
                // Dropped at once: only whether it opens counts.
                Ok(_) => probed.graphics_card = Some(encoder.name),
                Err(err) => tracing::debug!("{} is not usable here: {}", encoder.name, err.report()),
            }
        }
    }
    probed
}

/// The encoder this build has for a sound codec (FFmpeg's name), if any.
/// Quick: nothing is opened.
pub fn audio_encoder(codec: AudioCodec) -> Option<&'static str> {
    codec
        .encoders()
        .iter()
        .copied()
        .find(|name| find_encoder(name).is_some())
}

/// The names of the encoders this build has for `codec`, in the order tried.
/// Quick: nothing is opened.
pub fn encoders_present(codec: VideoCodec) -> Vec<&'static str> {
    codec
        .encoders()
        .iter()
        .map(|e| e.name)
        .filter(|name| find_encoder(name).is_some())
        .collect()
}

/// FFmpeg's long name for an encoder, for the log.
pub fn encoder_description(name: &str) -> Option<String> {
    let codec = find_encoder(name)?;
    // SAFETY: the codec and its long name are FFmpeg's static data when not null.
    unsafe {
        if (*codec).long_name.is_null() {
            return None;
        }
        Some(CStr::from_ptr((*codec).long_name).to_string_lossy().into_owned())
    }
}
