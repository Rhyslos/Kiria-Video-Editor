//! A thin FFmpeg video decoder for the caching measurements: software, or
//! the GPU's decoder with frames copied back to memory (the editor's current
//! path). Started from `spikes/decoding/src/decoder.rs`. Spike code: errors
//! are plain strings.

use ffmpeg_sys_next as ff;
use std::ffi::{CStr, CString};
use std::ptr;

pub type Result<T> = std::result::Result<T, String>;

/// Frames the decoder may leave out while racing to a target.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Skip {
    Nothing,
    /// Frames no other frame is predicted from.
    NonReference,
}

/// How a decoder is opened.
#[derive(Clone, Debug)]
pub struct Options {
    /// An FFmpeg device type (`d3d11va`, `vaapi`, ...) to decode on the GPU,
    /// copying frames back; `None` decodes on the CPU. A codec the GPU can't
    /// decode quietly falls back to the CPU, as in the editor.
    pub hardware: Option<String>,
    /// CPU threads; 0 lets FFmpeg choose (all of them).
    pub threads: i32,
    pub skip: Skip,
    /// Leave out the deblocking filter: faster, but pictures are no longer exact.
    pub skip_loop_filter: bool,
}

impl Options {
    pub fn new(hardware: &Option<String>) -> Options {
        Options {
            hardware: hardware.clone(),
            threads: 0,
            skip: Skip::Nothing,
            skip_loop_filter: false,
        }
    }

    /// One CPU thread: a frame comes out as soon as its packet went in.
    /// For proxies and picture indexes, where every fetch is one small frame.
    pub fn light() -> Options {
        Options {
            hardware: None,
            threads: 1,
            skip: Skip::Nothing,
            skip_loop_filter: false,
        }
    }
}

/// When a decoded frame is on screen: from `time` until `end`, in seconds
/// from the start of the file.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Stamp {
    pub time: f64,
    pub end: f64,
}

impl Stamp {
    pub fn covers(&self, t: f64) -> bool {
        self.time <= t && t < self.end
    }
}

/// How pixels are stored in a kept picture.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Pixels {
    /// Luma plus two quarter-size chroma planes: 1.5 bytes a pixel.
    Yuv420,
    /// 4 bytes a pixel, what the viewer takes today.
    Rgba,
}

/// A picture in memory at preview size.
#[derive(Clone, Debug)]
pub struct Picture {
    pub stamp: Stamp,
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
}

/// Bytes one picture takes.
pub fn picture_bytes(width: u32, height: u32, pixels: Pixels) -> usize {
    let px = width as usize * height as usize;
    match pixels {
        Pixels::Yuv420 => px + 2 * (width as usize / 2) * (height as usize / 2),
        Pixels::Rgba => px * 4,
    }
}

/// The size a `width × height` picture gets when fitted into `limit` (never
/// enlarged, even numbers), as the editor's decoder does.
pub fn fit(width: u32, height: u32, limit: (u32, u32)) -> (u32, u32) {
    let scale = (f64::from(limit.0) / f64::from(width))
        .min(f64::from(limit.1) / f64::from(height))
        .min(1.0);
    let even = |v: u32| ((f64::from(v) * scale).round() as u32).max(2) & !1;
    (even(width), even(height))
}

pub struct Decoder {
    format: *mut ff::AVFormatContext,
    codec: *mut ff::AVCodecContext,
    stream: i32,
    time_base: f64,
    /// Timestamp of the file's first frame, so times count from zero.
    start: f64,
    /// Length of one frame when a frame doesn't say.
    nominal: f64,
    packet: *mut ff::AVPacket,
    frame: *mut ff::AVFrame,
    copy: *mut ff::AVFrame,
    scaler: *mut ff::SwsContext,
    hw_format: Option<ff::AVPixelFormat>,
    hw_name: String,
    draining: bool,
    skip: Skip,
    /// With `Skip::NonReference`: frames from this time on are all decoded,
    /// so the target itself is never left out.
    exact_from: f64,
    /// The frame decoded last; `None` right after a seek.
    pub last: Option<Stamp>,
    /// Frames that came out of the decoder since it was opened.
    pub frames_out: u64,
    pub codec_name: String,
    pub duration: f64,
    pub width: u32,
    pub height: u32,
    pub fps: f64,
}

// The decoder is used by one thread at a time.
unsafe impl Send for Decoder {}

pub fn describe(code: i32, what: &str) -> String {
    let mut buf = [0 as std::ffi::c_char; 256];
    unsafe { ff::av_strerror(code, buf.as_mut_ptr(), buf.len()) };
    let text = unsafe { CStr::from_ptr(buf.as_ptr()) }.to_string_lossy();
    format!("{what}: {text}")
}

/// Picks the hardware format FFmpeg offers (stored in `opaque`), else the
/// first software format, so an unsupported codec decodes on the CPU.
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

fn rational(r: ff::AVRational) -> f64 {
    if r.den == 0 {
        0.0
    } else {
        f64::from(r.num) / f64::from(r.den)
    }
}

impl Decoder {
    pub fn open(path: &str, options: &Options) -> Result<Decoder> {
        unsafe {
            let cpath = CString::new(path).map_err(|e| e.to_string())?;
            let mut format = ptr::null_mut();
            let r = ff::avformat_open_input(&mut format, cpath.as_ptr(), ptr::null(), ptr::null_mut());
            if r < 0 {
                return Err(describe(r, "open"));
            }
            let r = ff::avformat_find_stream_info(format, ptr::null_mut());
            if r < 0 {
                ff::avformat_close_input(&mut format);
                return Err(describe(r, "stream info"));
            }
            let mut decoder: *const ff::AVCodec = ptr::null();
            let stream = ff::av_find_best_stream(
                format,
                ff::AVMediaType::AVMEDIA_TYPE_VIDEO,
                -1,
                -1,
                &mut decoder,
                0,
            );
            if stream < 0 {
                ff::avformat_close_input(&mut format);
                return Err(describe(stream, "no video stream"));
            }
            let st = *(*format).streams.add(stream as usize);
            let time_base = rational((*st).time_base);
            let codec = ff::avcodec_alloc_context3(decoder);
            let mut this = Decoder {
                format,
                codec,
                stream,
                time_base,
                start: 0.0,
                nominal: 1.0 / 60.0,
                packet: ff::av_packet_alloc(),
                frame: ff::av_frame_alloc(),
                copy: ff::av_frame_alloc(),
                scaler: ptr::null_mut(),
                hw_format: None,
                hw_name: String::new(),
                draining: false,
                skip: options.skip,
                exact_from: 0.0,
                last: None,
                frames_out: 0,
                codec_name: CStr::from_ptr((*decoder).name).to_string_lossy().into_owned(),
                duration: 0.0,
                width: (*(*st).codecpar).width.max(0) as u32,
                height: (*(*st).codecpar).height.max(0) as u32,
                fps: 60.0,
            };
            if (*st).start_time != ff::AV_NOPTS_VALUE {
                this.start = (*st).start_time as f64 * time_base;
            }
            let rate = rational((*st).avg_frame_rate);
            if rate > 0.0 {
                this.fps = rate;
                this.nominal = 1.0 / rate;
            }
            if (*format).duration > 0 {
                this.duration = (*format).duration as f64 / ff::AV_TIME_BASE as f64;
            }
            let r = ff::avcodec_parameters_to_context(codec, (*st).codecpar);
            if r < 0 {
                return Err(describe(r, "codec parameters"));
            }
            (*codec).thread_count = options.threads;
            (*codec).thread_type = (ff::FF_THREAD_FRAME | ff::FF_THREAD_SLICE) as i32;
            if options.skip_loop_filter {
                (*codec).skip_loop_filter = ff::AVDiscard::AVDISCARD_ALL;
            }
            if let Some(name) = &options.hardware {
                this.try_hardware(decoder, name);
            }
            let r = ff::avcodec_open2(codec, decoder, ptr::null_mut());
            if r < 0 {
                return Err(describe(r, "open codec"));
            }
            Ok(this)
        }
    }

    /// Asks FFmpeg for the GPU decoder; stays on the CPU when it isn't there.
    unsafe fn try_hardware(&mut self, decoder: *const ff::AVCodec, name: &str) {
        unsafe {
            let Ok(cname) = CString::new(name) else { return };
            let kind = ff::av_hwdevice_find_type_by_name(cname.as_ptr());
            if kind == ff::AVHWDeviceType::AV_HWDEVICE_TYPE_NONE {
                return;
            }
            let mut i = 0;
            let format = loop {
                let cfg = ff::avcodec_get_hw_config(decoder, i);
                if cfg.is_null() {
                    return;
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
                return;
            }
            (*self.codec).hw_device_ctx = device; // the context takes this reference
            (*self.codec).opaque = format as i32 as isize as *mut std::ffi::c_void;
            (*self.codec).get_format = Some(pick_format);
            self.hw_format = Some(format);
            self.hw_name = name.to_owned();
        }
    }

    /// "d3d11va" or "software": what decoded the frame held now.
    pub fn method(&self) -> String {
        let on_gpu = self
            .hw_format
            .is_some_and(|f| unsafe { (*self.frame).format } == f as i32);
        if on_gpu {
            self.hw_name.clone()
        } else {
            "software".into()
        }
    }

    fn stamp(&self) -> Stamp {
        unsafe {
            let ts = (*self.frame).best_effort_timestamp;
            let time = if ts == ff::AV_NOPTS_VALUE {
                self.last.map_or(0.0, |l| l.end)
            } else {
                ts as f64 * self.time_base - self.start
            };
            let length = (*self.frame).duration as f64 * self.time_base;
            let length = if length > 0.0 { length } else { self.nominal };
            Stamp {
                time,
                end: time + length,
            }
        }
    }

    /// Reads the next packet of the video stream into `self.packet`.
    /// Returns false at the end of the file.
    unsafe fn read_packet(&mut self) -> bool {
        unsafe {
            loop {
                if ff::av_read_frame(self.format, self.packet) < 0 {
                    return false;
                }
                if (*self.packet).stream_index == self.stream {
                    return true;
                }
                ff::av_packet_unref(self.packet);
            }
        }
    }

    /// Decodes the next frame, or `None` at the end. The frame stays in the
    /// decoder until the next call; [`Decoder::picture`] copies it out.
    pub fn next(&mut self) -> Result<Option<Stamp>> {
        unsafe {
            loop {
                ff::av_frame_unref(self.frame);
                let r = ff::avcodec_receive_frame(self.codec, self.frame);
                if r == 0 {
                    let stamp = self.stamp();
                    self.last = Some(stamp);
                    self.frames_out += 1;
                    return Ok(Some(stamp));
                }
                if r == ff::AVERROR_EOF || (r == ff::AVERROR(ff::EAGAIN) && self.draining) {
                    return Ok(None);
                }
                if r != ff::AVERROR(ff::EAGAIN) {
                    return Err(describe(r, "decode"));
                }
                if self.read_packet() {
                    if self.skip == Skip::NonReference {
                        let p = self.packet;
                        let ts = if (*p).pts != ff::AV_NOPTS_VALUE {
                            (*p).pts
                        } else {
                            (*p).dts
                        };
                        let racing = ts as f64 * self.time_base - self.start < self.exact_from;
                        (*self.codec).skip_frame = if racing {
                            ff::AVDiscard::AVDISCARD_NONREF
                        } else {
                            ff::AVDiscard::AVDISCARD_DEFAULT
                        };
                    }
                    let r = ff::avcodec_send_packet(self.codec, self.packet);
                    ff::av_packet_unref(self.packet);
                    if r < 0 && r != ff::AVERROR(ff::EAGAIN) {
                        return Err(describe(r, "send packet"));
                    }
                } else {
                    ff::avcodec_send_packet(self.codec, ptr::null());
                    self.draining = true;
                }
            }
        }
    }

    /// Moves to the key frame at or before `seconds`. The next frame decoded
    /// is that key frame.
    pub fn seek(&mut self, seconds: f64) -> Result<()> {
        unsafe {
            let ts = ((seconds.max(0.0) + self.start) / self.time_base) as i64;
            let r = ff::av_seek_frame(self.format, self.stream, ts, ff::AVSEEK_FLAG_BACKWARD as i32);
            if r < 0 {
                // Some files only seek to the start.
                let r = ff::av_seek_frame(self.format, self.stream, 0, ff::AVSEEK_FLAG_BACKWARD as i32);
                if r < 0 {
                    return Err(describe(r, "seek"));
                }
            }
            ff::avcodec_flush_buffers(self.codec);
            ff::av_frame_unref(self.frame);
        }
        self.draining = false;
        self.last = None;
        Ok(())
    }

    /// Seeks, then decodes on to the frame on screen at `seconds`.
    pub fn frame_at(&mut self, seconds: f64) -> Result<Option<Stamp>> {
        self.seek(seconds)?;
        self.exact_from = seconds - 1.5 * self.nominal;
        while let Some(stamp) = self.next()? {
            if seconds < stamp.end {
                return Ok(Some(stamp));
            }
        }
        Ok(None)
    }

    /// Seeks and decodes only the key frame at or before `seconds`: one
    /// packet in, then the decoder is told the stream ended so the picture
    /// comes out at once, whatever its threads or frame reordering would hold back.
    pub fn key_frame_at(&mut self, seconds: f64) -> Result<Option<Stamp>> {
        self.seek(seconds)?;
        unsafe {
            if !self.read_packet() {
                return Ok(None);
            }
            let r = ff::avcodec_send_packet(self.codec, self.packet);
            ff::av_packet_unref(self.packet);
            if r < 0 {
                return Err(describe(r, "send packet"));
            }
            ff::avcodec_send_packet(self.codec, ptr::null());
            self.draining = true;
        }
        self.next()
    }

    /// Copies the frame decoded last out of the decoder, scaled to fit `limit`.
    pub fn picture(&mut self, limit: (u32, u32), pixels: Pixels) -> Result<Picture> {
        let stamp = self.last.ok_or("no frame decoded yet")?;
        unsafe {
            let mut source = self.frame;
            if self.hw_format.is_some_and(|f| (*source).format == f as i32) {
                ff::av_frame_unref(self.copy);
                let r = ff::av_hwframe_transfer_data(self.copy, source, 0);
                if r < 0 {
                    return Err(describe(r, "copy from GPU"));
                }
                source = self.copy;
            }
            let (sw, sh) = ((*source).width, (*source).height);
            if sw <= 0 || sh <= 0 {
                return Err("a decoded picture had no size".into());
            }
            let (w, h) = fit(sw as u32, sh as u32, limit);
            let source_format = std::mem::transmute::<i32, ff::AVPixelFormat>((*source).format);
            let target_format = match pixels {
                Pixels::Yuv420 => ff::AVPixelFormat::AV_PIX_FMT_YUV420P,
                Pixels::Rgba => ff::AVPixelFormat::AV_PIX_FMT_RGBA,
            };
            self.scaler = ff::sws_getCachedContext(
                self.scaler,
                sw,
                sh,
                source_format,
                w as i32,
                h as i32,
                target_format,
                ff::SwsFlags::SWS_BILINEAR as i32,
                ptr::null_mut(),
                ptr::null_mut(),
                ptr::null(),
            );
            if self.scaler.is_null() {
                return Err("this picture format can't be converted".into());
            }
            let bytes = picture_bytes(w, h, pixels);
            // A little spare room: the scaler may write past the end of a row.
            let mut data = vec![0u8; bytes + 64];
            let base = data.as_mut_ptr();
            let luma = (w * h) as usize;
            let chroma = (w / 2 * (h / 2)) as usize;
            let (planes, strides) = match pixels {
                Pixels::Yuv420 => (
                    [base, base.add(luma), base.add(luma + chroma), ptr::null_mut()],
                    [w as i32, w as i32 / 2, w as i32 / 2, 0],
                ),
                Pixels::Rgba => (
                    [base, ptr::null_mut(), ptr::null_mut(), ptr::null_mut()],
                    [w as i32 * 4, 0, 0, 0],
                ),
            };
            ff::sws_scale(
                self.scaler,
                (*source).data.as_ptr() as *const *const u8,
                (*source).linesize.as_ptr(),
                0,
                sh,
                planes.as_ptr(),
                strides.as_ptr(),
            );
            data.truncate(bytes);
            Ok(Picture {
                stamp,
                width: w,
                height: h,
                data,
            })
        }
    }

    /// Key frame times the container lists without reading the file through
    /// (MP4's sample table, MKV's cues). Can be empty or incomplete.
    pub fn container_keys(&mut self, until: f64) -> Vec<f64> {
        // MKV reads its cues on the first seek.
        let _ = self.seek(0.0);
        let mut keys = Vec::new();
        unsafe {
            let st = *(*self.format).streams.add(self.stream as usize);
            for i in 0..ff::avformat_index_get_entries_count(st) {
                let e = ff::avformat_index_get_entry(st, i);
                if e.is_null() || (*e).flags() & ff::AVINDEX_KEYFRAME as i32 == 0 {
                    continue;
                }
                let t = (*e).timestamp as f64 * self.time_base - self.start;
                if t <= until {
                    keys.push(t);
                }
            }
        }
        keys.sort_by(f64::total_cmp);
        keys.dedup();
        keys
    }

    /// Key frame times found by reading every packet's header up to `until`
    /// seconds (no decoding). Exact, but reads the file through.
    pub fn scan_keys(&mut self, until: f64) -> Result<Vec<f64>> {
        self.seek(0.0)?;
        let mut keys = Vec::new();
        unsafe {
            while self.read_packet() {
                let p = self.packet;
                let ts = if (*p).pts != ff::AV_NOPTS_VALUE {
                    (*p).pts
                } else {
                    (*p).dts
                };
                let t = ts as f64 * self.time_base - self.start;
                let key = (*p).flags & ff::AV_PKT_FLAG_KEY as i32 != 0;
                ff::av_packet_unref(p);
                if t > until {
                    break;
                }
                if key {
                    keys.push(t);
                }
            }
        }
        keys.sort_by(f64::total_cmp);
        self.seek(0.0)?;
        Ok(keys)
    }
}

impl Drop for Decoder {
    fn drop(&mut self) {
        unsafe {
            ff::sws_freeContext(self.scaler);
            ff::av_frame_free(&mut self.frame);
            ff::av_frame_free(&mut self.copy);
            ff::av_packet_free(&mut self.packet);
            ff::avcodec_free_context(&mut self.codec);
            ff::avformat_close_input(&mut self.format);
        }
    }
}

/// Where a file's key frames are: decoding can only start at one of these.
#[derive(Clone, Debug)]
pub struct KeyIndex {
    pub keys: Vec<f64>,
}

impl KeyIndex {
    /// The key frame at or before `t` (the first one when `t` is before all).
    pub fn before(&self, t: f64) -> f64 {
        let i = self.keys.partition_point(|k| *k <= t + 1e-6);
        self.keys.get(i.saturating_sub(1)).copied().unwrap_or(0.0)
    }

    /// Distances between key frames, sorted.
    pub fn gaps(&self) -> Vec<f64> {
        let mut gaps: Vec<f64> = self.keys.windows(2).map(|w| w[1] - w[0]).collect();
        gaps.sort_by(f64::total_cmp);
        gaps
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fitting_never_enlarges_and_keeps_even_sizes() {
        assert_eq!(fit(2560, 1440, (1280, 720)), (1280, 720));
        assert_eq!(fit(1920, 1080, (1280, 720)), (1280, 720));
        assert_eq!(fit(960, 540, (1280, 720)), (960, 540));
        assert_eq!(fit(1000, 563, (500, 500)), (500, 282));
    }

    #[test]
    fn picture_sizes() {
        assert_eq!(picture_bytes(1280, 720, Pixels::Yuv420), 1_382_400);
        assert_eq!(picture_bytes(1280, 720, Pixels::Rgba), 3_686_400);
    }

    #[test]
    fn the_key_frame_before_a_time() {
        let index = KeyIndex {
            keys: vec![0.0, 2.0, 4.0],
        };
        assert_eq!(index.before(0.5), 0.0);
        assert_eq!(index.before(2.0), 2.0);
        assert_eq!(index.before(3.99), 2.0);
        assert_eq!(index.before(9.0), 4.0);
        assert_eq!(index.gaps(), vec![2.0, 2.0]);
    }
}
