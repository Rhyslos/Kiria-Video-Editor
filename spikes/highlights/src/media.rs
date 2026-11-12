//! Reading a recording with FFmpeg for the analysis: every audio stream
//! turned into mono samples at 16 kHz, and the video turned into tiny
//! pictures (64×36 brightness, 32×18 colour) in one pass. Spike code: errors
//! are plain strings, and the unsafe blocks are the FFmpeg calls.

// The FFmpeg constants' integer types differ between platforms' bindings.
#![allow(clippy::unnecessary_cast)]

use ffmpeg_sys_next as ff;
use std::ffi::{CStr, CString};
use std::ptr;

pub type Result<T> = std::result::Result<T, String>;

/// The rate every audio stream is turned into: plenty for loudness and
/// onsets, and cheap to look at.
pub const RATE: i32 = 16_000;

/// The size of the tiny pictures the video is analysed on.
pub const THUMB_W: usize = 64;
pub const THUMB_H: usize = 36;

fn describe(code: i32, what: &str) -> String {
    let mut buf = [0 as std::ffi::c_char; 256];
    unsafe { ff::av_strerror(code, buf.as_mut_ptr(), buf.len()) };
    let text = unsafe { CStr::from_ptr(buf.as_ptr()) }.to_string_lossy();
    format!("{what}: {text}")
}

fn rational(r: ff::AVRational) -> f64 {
    if r.den == 0 {
        0.0
    } else {
        f64::from(r.num) / f64::from(r.den)
    }
}

/// An open file. Closed on drop.
struct Input {
    format: *mut ff::AVFormatContext,
    /// The file's first timestamp, in seconds: times count from here, as a
    /// player shows them.
    start: f64,
}

impl Input {
    fn open(path: &str) -> Result<Input> {
        unsafe {
            let cpath = CString::new(path).map_err(|e| e.to_string())?;
            let mut format = ptr::null_mut();
            let r = ff::avformat_open_input(&mut format, cpath.as_ptr(), ptr::null(), ptr::null_mut());
            if r < 0 {
                return Err(describe(r, &format!("open {path}")));
            }
            let mut input = Input { format, start: 0.0 };
            let r = ff::avformat_find_stream_info(format, ptr::null_mut());
            if r < 0 {
                return Err(describe(r, "stream info"));
            }
            let start = (*format).start_time;
            if start != ff::AV_NOPTS_VALUE {
                input.start = start as f64 / f64::from(ff::AV_TIME_BASE);
            }
            Ok(input)
        }
    }

    fn duration(&self) -> f64 {
        let d = unsafe { (*self.format).duration };
        if d > 0 {
            d as f64 / f64::from(ff::AV_TIME_BASE)
        } else {
            0.0
        }
    }

    fn stream(&self, index: usize) -> *mut ff::AVStream {
        unsafe { *(*self.format).streams.add(index) }
    }

    fn streams(&self) -> usize {
        unsafe { (*self.format).nb_streams as usize }
    }

    fn kind(&self, index: usize) -> ff::AVMediaType {
        unsafe { (*(*self.stream(index)).codecpar).codec_type }
    }

    fn title(&self, index: usize) -> String {
        unsafe {
            let key = c"title";
            let entry = ff::av_dict_get((*self.stream(index)).metadata, key.as_ptr(), ptr::null(), 0);
            if entry.is_null() {
                String::new()
            } else {
                CStr::from_ptr((*entry).value).to_string_lossy().into_owned()
            }
        }
    }
}

impl Drop for Input {
    fn drop(&mut self) {
        unsafe { ff::avformat_close_input(&mut self.format) };
    }
}

/// A decoder for one stream. Freed on drop.
struct Codec {
    ctx: *mut ff::AVCodecContext,
}

impl Codec {
    fn open(
        input: &Input,
        index: usize,
        threads: i32,
        setup: impl FnOnce(*mut ff::AVCodecContext, *const ff::AVCodec),
    ) -> Result<Codec> {
        unsafe {
            let par = (*input.stream(index)).codecpar;
            let decoder = ff::avcodec_find_decoder((*par).codec_id);
            if decoder.is_null() {
                return Err(format!("no decoder for stream {index}"));
            }
            let ctx = ff::avcodec_alloc_context3(decoder);
            let codec = Codec { ctx };
            let r = ff::avcodec_parameters_to_context(ctx, par);
            if r < 0 {
                return Err(describe(r, "codec parameters"));
            }
            (*ctx).pkt_timebase = (*input.stream(index)).time_base;
            (*ctx).thread_count = threads;
            (*ctx).thread_type = (ff::FF_THREAD_FRAME | ff::FF_THREAD_SLICE) as i32;
            setup(ctx, decoder);
            let r = ff::avcodec_open2(ctx, decoder, ptr::null_mut());
            if r < 0 {
                return Err(describe(r, "open codec"));
            }
            Ok(codec)
        }
    }
}

impl Drop for Codec {
    fn drop(&mut self) {
        unsafe { ff::avcodec_free_context(&mut self.ctx) };
    }
}

/// A packet and a frame, freed on drop.
struct Buffers {
    packet: *mut ff::AVPacket,
    frame: *mut ff::AVFrame,
    soft: *mut ff::AVFrame,
}

impl Buffers {
    fn new() -> Buffers {
        unsafe {
            Buffers {
                packet: ff::av_packet_alloc(),
                frame: ff::av_frame_alloc(),
                soft: ff::av_frame_alloc(),
            }
        }
    }
}

impl Drop for Buffers {
    fn drop(&mut self) {
        unsafe {
            ff::av_packet_free(&mut self.packet);
            ff::av_frame_free(&mut self.frame);
            ff::av_frame_free(&mut self.soft);
        }
    }
}

/// One audio stream of a recording, as the file describes it.
#[derive(Clone, Debug)]
pub struct AudioStream {
    /// Counted among the audio streams only, from 0.
    pub number: usize,
    pub title: String,
    pub codec: String,
    pub channels: i32,
}

/// What a recording holds.
#[derive(Clone, Debug)]
pub struct Info {
    pub duration: f64,
    pub video: String,
    pub width: i32,
    pub height: i32,
    pub fps: f64,
    pub audio: Vec<AudioStream>,
}

pub fn info(path: &str) -> Result<Info> {
    let input = Input::open(path)?;
    let mut info = Info {
        duration: input.duration(),
        video: String::new(),
        width: 0,
        height: 0,
        fps: 0.0,
        audio: Vec::new(),
    };
    for i in 0..input.streams() {
        unsafe {
            let st = input.stream(i);
            let par = (*st).codecpar;
            let name = CStr::from_ptr(ff::avcodec_get_name((*par).codec_id))
                .to_string_lossy()
                .into_owned();
            match input.kind(i) {
                ff::AVMediaType::AVMEDIA_TYPE_VIDEO if info.video.is_empty() => {
                    info.video = name;
                    info.width = (*par).width;
                    info.height = (*par).height;
                    info.fps = rational((*st).avg_frame_rate);
                }
                ff::AVMediaType::AVMEDIA_TYPE_AUDIO => info.audio.push(AudioStream {
                    number: info.audio.len(),
                    title: input.title(i),
                    codec: name,
                    channels: (*par).ch_layout.nb_channels,
                }),
                _ => {}
            }
        }
    }
    Ok(info)
}

/// Receives the audio of every stream, in order, as mono samples at
/// [`RATE`]. `begin` is called once per stream with the time of its first
/// sample.
pub trait AudioSink {
    fn begin(&mut self, stream: usize, time: f64);
    fn samples(&mut self, stream: usize, samples: &[f32]);
}

struct AudioDecoder {
    stream: usize,
    number: usize,
    codec: Codec,
    resampler: *mut ff::SwrContext,
    time_base: f64,
    started: bool,
}

impl Drop for AudioDecoder {
    fn drop(&mut self) {
        unsafe { ff::swr_free(&mut self.resampler) };
    }
}

impl AudioDecoder {
    unsafe fn resampler_for(&mut self, frame: *const ff::AVFrame) -> Result<()> {
        unsafe {
            if !self.resampler.is_null() {
                return Ok(());
            }
            let mut mono: ff::AVChannelLayout = std::mem::zeroed();
            ff::av_channel_layout_default(&mut mono, 1);
            let r = ff::swr_alloc_set_opts2(
                &mut self.resampler,
                &mono,
                ff::AVSampleFormat::AV_SAMPLE_FMT_FLT,
                RATE,
                &(*frame).ch_layout,
                std::mem::transmute::<i32, ff::AVSampleFormat>((*frame).format),
                (*frame).sample_rate,
                0,
                ptr::null_mut(),
            );
            if r < 0 {
                return Err(describe(r, "resampler"));
            }
            let r = ff::swr_init(self.resampler);
            if r < 0 {
                return Err(describe(r, "resampler init"));
            }
            Ok(())
        }
    }

    /// Takes every frame the decoder has ready and hands it on.
    unsafe fn drain(
        &mut self,
        start: f64,
        buffers: &Buffers,
        out: &mut Vec<f32>,
        sink: &mut dyn AudioSink,
    ) -> Result<()> {
        unsafe {
            loop {
                let r = ff::avcodec_receive_frame(self.codec.ctx, buffers.frame);
                if r == ff::AVERROR(ff::EAGAIN) || r == ff::AVERROR_EOF {
                    return Ok(());
                }
                if r < 0 {
                    return Err(describe(r, "decode audio"));
                }
                let frame = buffers.frame;
                self.resampler_for(frame)?;
                if !self.started {
                    self.started = true;
                    let ts = (*frame).best_effort_timestamp;
                    let time = if ts == ff::AV_NOPTS_VALUE {
                        0.0
                    } else {
                        ts as f64 * self.time_base - start
                    };
                    sink.begin(self.number, time);
                }
                let room = ff::swr_get_out_samples(self.resampler, (*frame).nb_samples).max(0) as usize + 32;
                out.resize(room, 0.0);
                let dst = out.as_mut_ptr() as *mut u8;
                let n = ff::swr_convert(
                    self.resampler,
                    &dst,
                    room as i32,
                    (*frame).extended_data as *const *const u8,
                    (*frame).nb_samples,
                );
                ff::av_frame_unref(frame);
                if n < 0 {
                    return Err(describe(n, "resample"));
                }
                sink.samples(self.number, &out[..n as usize]);
            }
        }
    }
}

/// Decodes every audio stream of `path` in one pass through the file, up to
/// `limit` seconds. Returns the number of seconds read.
pub fn read_audio(path: &str, limit: f64, sink: &mut dyn AudioSink) -> Result<f64> {
    let input = Input::open(path)?;
    let mut decoders: Vec<AudioDecoder> = Vec::new();
    for i in 0..input.streams() {
        unsafe {
            if input.kind(i) == ff::AVMediaType::AVMEDIA_TYPE_AUDIO {
                let codec = Codec::open(&input, i, 1, |_, _| {})?;
                decoders.push(AudioDecoder {
                    stream: i,
                    number: decoders.len(),
                    codec,
                    resampler: ptr::null_mut(),
                    time_base: rational((*input.stream(i)).time_base),
                    started: false,
                });
            } else {
                // The demuxer still reads these packets, but nothing decodes them.
                (*input.stream(i)).discard = ff::AVDiscard::AVDISCARD_ALL;
            }
        }
    }
    if decoders.is_empty() {
        return Err("no audio streams".into());
    }
    let buffers = Buffers::new();
    let mut out = Vec::new();
    let mut seen = 0.0f64;
    unsafe {
        while ff::av_read_frame(input.format, buffers.packet) >= 0 {
            let p = buffers.packet;
            let index = (*p).stream_index as usize;
            if let Some(d) = decoders.iter_mut().find(|d| d.stream == index) {
                if (*p).pts != ff::AV_NOPTS_VALUE {
                    seen = seen.max((*p).pts as f64 * d.time_base - input.start);
                }
                let r = ff::avcodec_send_packet(d.codec.ctx, p);
                ff::av_packet_unref(p);
                if r < 0 && r != ff::AVERROR(ff::EAGAIN) && r != ff::AVERROR_INVALIDDATA {
                    return Err(describe(r, "send audio packet"));
                }
                d.drain(input.start, &buffers, &mut out, sink)?;
            } else {
                ff::av_packet_unref(p);
            }
            if seen > limit {
                break;
            }
        }
        for d in &mut decoders {
            ff::avcodec_send_packet(d.codec.ctx, ptr::null());
            d.drain(input.start, &buffers, &mut out, sink)?;
        }
    }
    Ok(seen.min(limit))
}

/// Which frames the video pass decodes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Frames {
    /// Every frame.
    All,
    /// Every frame, without the deblocking filters: faster, and the small
    /// pictures barely change.
    AllNoDeblock,
    /// Key frames only: the packets in between are read but never decoded.
    Keys,
}

/// How the video pass decodes.
#[derive(Clone, Debug)]
pub struct VideoOptions {
    /// An FFmpeg device type (`d3d11va`, `vaapi`, ...) to decode on the GPU,
    /// copying frames back; `None` decodes on the CPU. A codec the GPU can't
    /// decode falls back to the CPU.
    pub hardware: Option<String>,
    /// Decoder threads; 0 lets FFmpeg choose (all of them).
    pub threads: i32,
    pub frames: Frames,
}

/// A tiny picture of one frame: brightness 64×36, colour 32×18, 8 bits.
#[derive(Clone, Debug, PartialEq)]
pub struct Thumb {
    pub time: f64,
    pub y: Vec<u8>,
    pub u: Vec<u8>,
    pub v: Vec<u8>,
}

/// What a video pass did.
#[derive(Clone, Debug, Default)]
pub struct VideoStats {
    pub frames: u64,
    /// "software" or the device that decoded the frames.
    pub method: String,
    pub seconds: f64,
}

unsafe extern "C" fn pick_format(
    ctx: *mut ff::AVCodecContext,
    offered: *const ff::AVPixelFormat,
) -> ff::AVPixelFormat {
    unsafe {
        let wanted = (*ctx).opaque as isize as i32;
        let mut first_software = ff::AVPixelFormat::AV_PIX_FMT_NONE;
        let mut p = offered;
        while *p != ff::AVPixelFormat::AV_PIX_FMT_NONE {
            if *p as i32 == wanted {
                return *p;
            }
            let desc = ff::av_pix_fmt_desc_get(*p);
            let hw = !desc.is_null() && (*desc).flags & (ff::AV_PIX_FMT_FLAG_HWACCEL as u64) != 0;
            if !hw && first_software == ff::AVPixelFormat::AV_PIX_FMT_NONE {
                first_software = *p;
            }
            p = p.add(1);
        }
        first_software
    }
}

/// Asks FFmpeg for the GPU decoder; stays on the CPU when it isn't there.
unsafe fn try_hardware(
    ctx: *mut ff::AVCodecContext,
    decoder: *const ff::AVCodec,
    name: &str,
) -> Option<ff::AVPixelFormat> {
    unsafe {
        let cname = CString::new(name).ok()?;
        let kind = ff::av_hwdevice_find_type_by_name(cname.as_ptr());
        if kind == ff::AVHWDeviceType::AV_HWDEVICE_TYPE_NONE {
            return None;
        }
        let mut i = 0;
        let format = loop {
            let cfg = ff::avcodec_get_hw_config(decoder, i);
            if cfg.is_null() {
                return None;
            }
            if (*cfg).device_type == kind
                && (*cfg).methods & ff::AV_CODEC_HW_CONFIG_METHOD_HW_DEVICE_CTX as i32 != 0
            {
                break (*cfg).pix_fmt;
            }
            i += 1;
        };
        let mut device = ptr::null_mut();
        if ff::av_hwdevice_ctx_create(&mut device, kind, ptr::null(), ptr::null_mut(), 0) < 0 {
            return None;
        }
        (*ctx).hw_device_ctx = device; // the context takes this reference
        (*ctx).opaque = format as i32 as isize as *mut std::ffi::c_void;
        (*ctx).get_format = Some(pick_format);
        Some(format)
    }
}

/// Reads one component (0 brightness, 1 and 2 colour) of a YUV frame into a
/// `w × h` grid of block averages, each from an 8×8 grid of samples inside
/// its block. Works for every planar and semi-planar YUV layout and bit
/// depth FFmpeg describes (8-bit 4:2:0, NV12, P010, 10-bit planar...).
unsafe fn shrink(
    frame: *const ff::AVFrame,
    desc: *const ff::AVPixFmtDescriptor,
    comp: usize,
    w: usize,
    h: usize,
    out: &mut Vec<u8>,
) {
    unsafe {
        let c = (*desc).comp[comp];
        let (sx, sy) = if comp == 0 {
            (0, 0)
        } else {
            ((*desc).log2_chroma_w, (*desc).log2_chroma_h)
        };
        let pw = (((*frame).width + (1 << sx) - 1) >> sx) as usize;
        let ph = (((*frame).height + (1 << sy) - 1) >> sy) as usize;
        let base = (*frame).data[c.plane as usize];
        let stride = (*frame).linesize[c.plane as usize] as isize;
        let wide = c.depth > 8;
        let down = c.depth - 8;
        out.clear();
        const SAMPLES: usize = 8;
        for by in 0..h {
            for bx in 0..w {
                let mut sum = 0u32;
                for j in 0..SAMPLES {
                    let y = (by * SAMPLES + j) * ph / (h * SAMPLES);
                    let row = base.offset(y as isize * stride);
                    for i in 0..SAMPLES {
                        let x = (bx * SAMPLES + i) * pw / (w * SAMPLES);
                        let at = row.add(x * c.step as usize + c.offset as usize);
                        let v = if wide {
                            let raw = u16::from_le_bytes([*at, *at.add(1)]);
                            u32::from(raw >> c.shift) >> down
                        } else {
                            u32::from(*at)
                        };
                        sum += v.min(255);
                    }
                }
                out.push((sum / (SAMPLES * SAMPLES) as u32) as u8);
            }
        }
    }
}

/// Decodes the video of `path` up to `limit` seconds (or only between
/// `from` and `to` when given) and hands every decoded frame on as a tiny
/// picture.
pub fn read_video(
    path: &str,
    options: &VideoOptions,
    from: Option<f64>,
    to: f64,
    on_frame: &mut dyn FnMut(Thumb),
) -> Result<VideoStats> {
    let input = Input::open(path)?;
    let stream = unsafe {
        ff::av_find_best_stream(
            input.format,
            ff::AVMediaType::AVMEDIA_TYPE_VIDEO,
            -1,
            -1,
            ptr::null_mut(),
            0,
        )
    };
    if stream < 0 {
        return Err(describe(stream, "no video stream"));
    }
    let stream = stream as usize;
    let mut hw_format = None;
    let codec = Codec::open(&input, stream, options.threads, |ctx, decoder| unsafe {
        if options.frames == Frames::AllNoDeblock {
            (*ctx).skip_loop_filter = ff::AVDiscard::AVDISCARD_ALL;
        }
        if let Some(name) = &options.hardware {
            hw_format = try_hardware(ctx, decoder, name);
        }
    })?;
    let time_base = unsafe {
        for i in 0..input.streams() {
            if i != stream {
                (*input.stream(i)).discard = ff::AVDiscard::AVDISCARD_ALL;
            }
        }
        rational((*input.stream(stream)).time_base)
    };
    if let Some(from) = from {
        let ts = ((from + input.start) / time_base) as i64;
        let r =
            unsafe { ff::av_seek_frame(input.format, stream as i32, ts, ff::AVSEEK_FLAG_BACKWARD as i32) };
        if r < 0 {
            return Err(describe(r, "seek"));
        }
    }
    let buffers = Buffers::new();
    let mut stats = VideoStats {
        method: "software".into(),
        ..VideoStats::default()
    };
    // Hands on every frame the decoder has ready; true once past `to`.
    let mut receive = |stats: &mut VideoStats| -> Result<bool> {
        let mut done = false;
        unsafe {
            loop {
                let r = ff::avcodec_receive_frame(codec.ctx, buffers.frame);
                if r == ff::AVERROR(ff::EAGAIN) || r == ff::AVERROR_EOF {
                    return Ok(done);
                }
                if r < 0 {
                    return Err(describe(r, "decode video"));
                }
                let mut frame = buffers.frame;
                let ts = (*frame).best_effort_timestamp;
                let time = if ts == ff::AV_NOPTS_VALUE {
                    stats.seconds
                } else {
                    ts as f64 * time_base - input.start
                };
                if hw_format.is_some_and(|f| (*frame).format == f as i32) {
                    let r = ff::av_hwframe_transfer_data(buffers.soft, frame, 0);
                    if r < 0 {
                        return Err(describe(r, "copy frame from the GPU"));
                    }
                    stats.method = options.hardware.clone().unwrap_or_default();
                    frame = buffers.soft;
                }
                if time >= to {
                    done = true;
                } else if from.is_none_or(|f| time >= f - 1e-6) {
                    let format = std::mem::transmute::<i32, ff::AVPixelFormat>((*frame).format);
                    let desc = ff::av_pix_fmt_desc_get(format);
                    if desc.is_null()
                        || (*desc).flags & (ff::AV_PIX_FMT_FLAG_RGB as u64) != 0
                        || (*desc).nb_components < 3
                    {
                        return Err(format!(
                            "frames in a layout the spike can't read ({})",
                            (*frame).format
                        ));
                    }
                    let mut thumb = Thumb {
                        time,
                        y: Vec::new(),
                        u: Vec::new(),
                        v: Vec::new(),
                    };
                    shrink(frame, desc, 0, THUMB_W, THUMB_H, &mut thumb.y);
                    shrink(frame, desc, 1, THUMB_W / 2, THUMB_H / 2, &mut thumb.u);
                    shrink(frame, desc, 2, THUMB_W / 2, THUMB_H / 2, &mut thumb.v);
                    stats.frames += 1;
                    stats.seconds = stats.seconds.max(time);
                    on_frame(thumb);
                }
                ff::av_frame_unref(buffers.soft);
                ff::av_frame_unref(buffers.frame);
            }
        }
    };
    let mut done = false;
    unsafe {
        while !done && ff::av_read_frame(input.format, buffers.packet) >= 0 {
            let p = buffers.packet;
            let wanted = (*p).stream_index as usize == stream
                && (options.frames != Frames::Keys || (*p).flags & ff::AV_PKT_FLAG_KEY as i32 != 0);
            if wanted {
                let r = ff::avcodec_send_packet(codec.ctx, p);
                ff::av_packet_unref(p);
                if r < 0 && r != ff::AVERROR(ff::EAGAIN) && r != ff::AVERROR_INVALIDDATA {
                    return Err(describe(r, "send video packet"));
                }
                done = receive(&mut stats)?;
            } else {
                ff::av_packet_unref(p);
            }
        }
        if !done {
            ff::avcodec_send_packet(codec.ctx, ptr::null());
            receive(&mut stats)?;
        }
    }
    Ok(stats)
}
