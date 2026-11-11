//! Turning a video stream into pictures: the frame shown at any moment.
//!
//! [`VideoDecoder::frame_at`] returns the frame on screen at a time in the
//! file: the last frame whose timestamp is at or before it. Frames are found
//! by timestamp, never by counting, so variable-frame-rate recordings land
//! right and stay in step with their sound.
//!
//! Moving forward a little decodes onward from where it is; jumping back or
//! far ahead seeks to the key frame before the target first.
//!
//! The picture comes out one of two ways. [`VideoDecoder::picture_at`] hands
//! over the decoder's own YUV planes ([`crate::yuv`]) for the GPU to convert
//! while drawing, whenever the format is the common 8-bit 4:2:0, and falls
//! back to RGBA for every other format. [`VideoDecoder::frame_at`] always
//! gives RGBA, scaled to the size asked for and turned upright, on the CPU.
//!
//! There are three ways to decode, fastest first (`docs/research/decoding.md`):
//!
//! - on the window's own graphics device ([`VideoDecoder::open_on_device`],
//!   [`crate::vulkan`]): the picture stays on the graphics card;
//! - on the GPU's video decoder with a copy back to memory (Direct3D 11 on
//!   Windows, VA-API on Linux);
//! - on the CPU.
//!
//! [`VideoDecoder::open`] with [`Decoding::Best`] takes the second when it
//! can open and handles the codec, else the third. The preview tries the
//! first before that and steps down per file when a way fails.

use crate::ffi::{Input, error_text};
use crate::jump::KeyFrames;
use crate::vulkan::{GpuFrame, WindowDevice, device_error};
use crate::yuv::{Colour, PlaneRef, Pool, Source, YuvFrame, step_for};
use diag::AppError;
use ffmpeg_sys_next as ff;
use std::path::Path;
use std::ptr;
use std::sync::Arc;
use std::time::{Duration, Instant};
use video_device::Copier;

/// A decoded picture, upright, RGBA, `width × height`.
#[derive(Clone, PartialEq)]
pub struct Frame {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
    /// When it starts, in seconds from the start of the file.
    pub time: f64,
}

impl std::fmt::Debug for Frame {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Frame({}×{} at {:.3} s)", self.width, self.height, self.time)
    }
}

/// The picture at a moment, as [`VideoDecoder::picture_at`] could make it.
#[derive(Debug)]
pub enum Decoded {
    /// The decoder's planes: the GPU converts them while drawing.
    Yuv(YuvFrame),
    /// Converted on the CPU, for formats the planes path doesn't take.
    Rgba(Frame),
    /// Decoded on the window's graphics device and still there: two images
    /// the GPU converts while drawing.
    Gpu(GpuFrame),
}

impl Decoded {
    /// When the picture starts, in seconds from the start of the file.
    pub fn time(&self) -> f64 {
        match self {
            Decoded::Yuv(frame) => frame.time,
            Decoded::Rgba(frame) => frame.time,
            Decoded::Gpu(frame) => frame.time,
        }
    }

    /// The size on screen, upright.
    pub fn size(&self) -> (u32, u32) {
        match self {
            Decoded::Yuv(frame) => frame.upright_size(),
            Decoded::Rgba(frame) => (frame.width, frame.height),
            Decoded::Gpu(frame) => frame.upright_size(),
        }
    }
}

/// Which decoder to use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decoding {
    /// The GPU's video decoder when it works, else the CPU.
    Best,
    /// Always the CPU.
    Software,
}

/// The way a decoder works, fastest first. A file that fails one way is
/// opened again one step down.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    /// On the window's own graphics device; pictures stay on the card.
    Device,
    /// On the GPU's video decoder, each picture copied back to memory.
    Hardware,
    /// On the CPU.
    Software,
}

/// What has been asked for since a decode began, as far as its file goes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Newer {
    /// Nothing: carry on.
    Nothing,
    /// Another moment of the same file (seconds from its start).
    /// `give_up` says whether the decode under way may be given up for it
    /// when it can't serve it. When not, it is finished first: whoever
    /// asks decides how often decodes may be thrown away, so that a
    /// steady stream of requests still gets pictures.
    At { moment: f64, give_up: bool },
    /// Something this decoder can't give: another file, nothing at all.
    /// The decode is given up.
    Elsewhere,
}

/// The most a frame's stored time is taken to be off by (seconds), however
/// coarse the stream's clock: well under a frame at any usual frame rate.
const LARGEST_TICK: f64 = 0.002;

/// Where the time of the last picture went: for finding out why one was slow.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Work {
    /// The decoder jumped in the file first (otherwise it decoded on).
    pub jumped: bool,
    /// The jump itself: finding the place and emptying the decoder.
    pub jump: Duration,
    /// From the start until the first frame came out of the decoder.
    pub first_frame: Option<Duration>,
    /// Frames decoded.
    pub frames: u32,
    /// All of it, up to the frame being held.
    pub decoding: Duration,
}

/// How a decode that may be overtaken by a newer request ended.
#[derive(Debug)]
pub enum Reached<T> {
    /// The picture at `at`: the moment asked for, or the newer one when
    /// that turned out to be on the way before it.
    Shown { picture: T, at: f64 },
    /// Given up: what was asked for meanwhile can't use this decode. The
    /// decoder is where it had got to; ask it for the newer moment.
    Dropped,
}

pub struct VideoDecoder {
    input: Input,
    codec: *mut ff::AVCodecContext,
    stream: i32,
    time_base: f64,
    /// Timestamp of the file's first frame, so times count from zero.
    start: f64,
    rotation: u16,
    hw_format: Option<ff::AVPixelFormat>,
    /// The frame shown at the last time asked for.
    held: *mut ff::AVFrame,
    held_time: Option<f64>,
    /// The next frame, decoded already.
    ahead: *mut ff::AVFrame,
    ahead_time: Option<f64>,
    /// Where the container says the key frames are.
    keys: KeyFrames,
    /// How far a frame's stored time may be from its true time (seconds):
    /// half a step of the stream's clock. A recording that counts in
    /// milliseconds stores a frame due at 16.667 ms as 17; it is still the
    /// frame on screen at 16.667.
    tick: f64,
    /// Where the time of the last picture went.
    work: Work,
    scratch: *mut ff::AVFrame,
    copy: *mut ff::AVFrame,
    packet: *mut ff::AVPacket,
    scaler: *mut ff::SwsContext,
    /// Plane buffers that came back from pictures no longer shown.
    pool: Pool,
    /// Decoding on the window's device: the shared device, and what copies
    /// pictures out of the decoder's images.
    on_device: Option<(Arc<WindowDevice>, Copier)>,
    ended: bool,
    /// "hardware (d3d11va)" or "software", for the log.
    pub method: String,
}

impl std::fmt::Debug for VideoDecoder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "VideoDecoder({}, at {:?})", self.method, self.held_time)
    }
}

// A decoder is used by one thread at a time; FFmpeg contexts may move between threads.
unsafe impl Send for VideoDecoder {}

fn explain(path: &Path, what: &str, code: i32) -> AppError {
    AppError::new("The video can't be shown")
        .why(format!(
            "{what} failed for {} ({}).",
            path.display(),
            error_text(code)
        ))
        .fix("Check that the file still plays in a media player. If it does, report it with the file.")
}

/// Picks the hardware format FFmpeg offers (stored in `opaque`), else the
/// first software format, so an unsupported codec quietly decodes on the CPU.
unsafe extern "C" fn pick_format(
    ctx: *mut ff::AVCodecContext,
    offered: *const ff::AVPixelFormat,
) -> ff::AVPixelFormat {
    // SAFETY: FFmpeg passes a valid context and a list ended by AV_PIX_FMT_NONE.
    unsafe {
        let wanted = (*ctx).opaque as isize as i32;
        let mut first_software = ff::AVPixelFormat::AV_PIX_FMT_NONE;
        let mut p = offered;
        while *p != ff::AVPixelFormat::AV_PIX_FMT_NONE {
            if *p as i32 == wanted {
                return *p;
            }
            let desc = ff::av_pix_fmt_desc_get(*p);
            let hw = !desc.is_null() && (*desc).flags & u64::from(ff::AV_PIX_FMT_FLAG_HWACCEL as u32) != 0;
            if !hw && first_software == ff::AVPixelFormat::AV_PIX_FMT_NONE {
                first_software = *p;
            }
            p = p.add(1);
        }
        first_software
    }
}

/// Picks the hardware format stored in `opaque` when FFmpeg offers it, and
/// nothing otherwise, which makes FFmpeg report an error instead of quietly
/// decoding on the CPU.
unsafe extern "C" fn pick_format_or_none(
    ctx: *mut ff::AVCodecContext,
    offered: *const ff::AVPixelFormat,
) -> ff::AVPixelFormat {
    // SAFETY: FFmpeg passes a valid context and a list ended by AV_PIX_FMT_NONE.
    unsafe {
        let wanted = (*ctx).opaque as isize as i32;
        let mut p = offered;
        while *p != ff::AVPixelFormat::AV_PIX_FMT_NONE {
            if *p as i32 == wanted {
                return *p;
            }
            p = p.add(1);
        }
        ff::AVPixelFormat::AV_PIX_FMT_NONE
    }
}

/// Whether pictures in this format (a stream's, as its header gives it) can
/// be drawn from the graphics card by the viewer's shader: 8 bits with
/// colour at half size both ways. A format the header doesn't give is let
/// through; the decoded picture is checked again.
fn shader_takes(format: i32) -> bool {
    [
        ff::AVPixelFormat::AV_PIX_FMT_YUV420P,
        ff::AVPixelFormat::AV_PIX_FMT_YUVJ420P,
        ff::AVPixelFormat::AV_PIX_FMT_NONE,
    ]
    .iter()
    .any(|taken| *taken as i32 == format)
}

/// How to open a decoder.
#[derive(Clone, Copy)]
enum Way<'a> {
    /// On the window's own graphics device, or not at all.
    Device(&'a Arc<WindowDevice>),
    /// On the GPU's video decoder with a copy back, else on the CPU.
    Best,
    Software,
}

const HARDWARE: &str = if cfg!(windows) { "d3d11va" } else { "vaapi" };

/// The FFmpeg build loads the GPU decoding libraries on first use and stops
/// the program if they are missing, so check first. On Windows the
/// Direct3D 11 libraries are always there; on Linux VA-API may not be installed.
/// The GPU decoding interface for this system, if FFmpeg and the system
/// have it.
fn hardware_kind() -> Option<ff::AVHWDeviceType> {
    if !hardware_libraries_present() {
        return None;
    }
    let name = std::ffi::CString::new(HARDWARE).ok()?;
    // SAFETY: a plain lookup by name.
    let kind = unsafe { ff::av_hwdevice_find_type_by_name(name.as_ptr()) };
    (kind != ff::AVHWDeviceType::AV_HWDEVICE_TYPE_NONE).then_some(kind)
}

/// A decoder for `codec` that can decode on the GPU through this system's
/// interface, and the frame format it delivers there. FFmpeg can have
/// several decoders for one codec; the default one isn't always the one
/// with GPU support (for AV1 it is a software decoder).
pub(crate) fn hardware_decoder(codec: ff::AVCodecID) -> Option<(*const ff::AVCodec, ff::AVPixelFormat)> {
    decoder_for(codec, hardware_kind()?)
}

/// A decoder for `codec` that can decode through the GPU interface `kind`.
fn decoder_for(
    codec: ff::AVCodecID,
    kind: ff::AVHWDeviceType,
) -> Option<(*const ff::AVCodec, ff::AVPixelFormat)> {
    // SAFETY: iterating FFmpeg's static list of codecs and their static
    // hardware configurations; nothing is kept but pointers to them.
    unsafe {
        let mut opaque = ptr::null_mut();
        loop {
            let c = ff::av_codec_iterate(&mut opaque);
            if c.is_null() {
                return None;
            }
            if (*c).id != codec || ff::av_codec_is_decoder(c) == 0 {
                continue;
            }
            let mut i = 0;
            loop {
                let cfg = ff::avcodec_get_hw_config(c, i);
                if cfg.is_null() {
                    break;
                }
                if (*cfg).device_type == kind
                    && (*cfg).methods & ff::AV_CODEC_HW_CONFIG_METHOD_HW_DEVICE_CTX as i32 != 0
                {
                    return Some((c, (*cfg).pix_fmt));
                }
                i += 1;
            }
        }
    }
}

fn hardware_libraries_present() -> bool {
    if cfg!(windows) {
        return true;
    }
    static PRESENT: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *PRESENT.get_or_init(|| {
        ["libva.so.2", "libva-drm.so.2"].iter().all(|name| {
            // SAFETY: loading these libraries runs only their initialisers,
            // which have no requirements; they are closed straight away.
            unsafe { libloading::Library::new(name) }.is_ok()
        })
    })
}

impl VideoDecoder {
    /// Opens stream `stream` of the file. `rotation` is the clockwise turn
    /// that shows it upright (from probing).
    pub fn open(path: &Path, stream: u32, rotation: u16, decoding: Decoding) -> diag::Result<VideoDecoder> {
        let way = match decoding {
            Decoding::Best => Way::Best,
            Decoding::Software => Way::Software,
        };
        VideoDecoder::open_as(path, stream, rotation, way)
    }

    /// Opens stream `stream` of the file for decoding on the window's own
    /// graphics device. Fails, with the reason, when the device doesn't
    /// decode this kind of video or FFmpeg can't set it up: open the file
    /// with [`VideoDecoder::open`] then.
    pub fn open_on_device(
        path: &Path,
        stream: u32,
        rotation: u16,
        device: &Arc<WindowDevice>,
    ) -> diag::Result<VideoDecoder> {
        VideoDecoder::open_as(path, stream, rotation, Way::Device(device))
    }

    /// How this decoder works.
    pub fn level(&self) -> Level {
        if self.on_device.is_some() {
            Level::Device
        } else if self.hw_format.is_some() {
            Level::Hardware
        } else {
            Level::Software
        }
    }

    fn open_as(path: &Path, stream: u32, rotation: u16, way: Way<'_>) -> diag::Result<VideoDecoder> {
        let input = Input::open(path).map_err(|e| {
            AppError::new("The video can't be shown")
                .why(format!("{}: {}.", path.display(), e.text))
                .fix("Check that the file still exists and that the drive is connected.")
        })?;
        let raw = input.raw();
        // SAFETY: `raw` is a valid, opened format context owned by `input`;
        // every pointer below comes from FFmpeg and is checked before use.
        unsafe {
            if stream >= (*raw).nb_streams {
                return Err(AppError::new("The video can't be shown")
                    .why(format!("{} has no stream {stream}.", path.display()))
                    .fix("Import the file again."));
            }
            let st = *(*raw).streams.add(stream as usize);
            let par = (*st).codecpar;
            // The decoder that can use the GPU, when hardware is wanted: for
            // AV1 that is FFmpeg's own decoder, not the default one, which is
            // software only.
            let hardware = match way {
                Way::Device(device) => {
                    if !device.decodes((*par).codec_id) {
                        return Err(device_error(
                            "The device's decoder doesn't take this kind of video.",
                        ));
                    }
                    if !shader_takes((*par).format) {
                        return Err(device_error(
                            "The picture isn't 8-bit with colour at half size (10-bit and the like go the usual way).",
                        ));
                    }
                    let vulkan = ff::AVHWDeviceType::AV_HWDEVICE_TYPE_VULKAN;
                    let found = decoder_for((*par).codec_id, vulkan).ok_or_else(|| {
                        device_error("This FFmpeg build has no Vulkan decoder for this kind of video.")
                    })?;
                    Some(found)
                }
                Way::Best => hardware_decoder((*par).codec_id),
                Way::Software => None,
            };
            let decoder = match hardware {
                Some((d, _)) => d,
                None => ff::avcodec_find_decoder((*par).codec_id),
            };
            if decoder.is_null() {
                return Err(AppError::new("The video can't be shown")
                    .why(format!(
                        "No decoder for this kind of video in {}.",
                        path.display()
                    ))
                    .fix("Convert the file to H.264, H.265 or AV1 first."));
            }
            let codec = ff::avcodec_alloc_context3(decoder);
            if codec.is_null() {
                return Err(explain(path, "Setting up the decoder", ff::AVERROR(ff::ENOMEM)));
            }
            let mut this = VideoDecoder {
                input,
                codec,
                stream: stream as i32,
                time_base: f64::from((*st).time_base.num) / f64::from((*st).time_base.den.max(1)),
                start: 0.0,
                rotation,
                hw_format: None,
                held: ff::av_frame_alloc(),
                held_time: None,
                ahead: ff::av_frame_alloc(),
                ahead_time: None,
                keys: KeyFrames::default(),
                tick: 1e-6,
                work: Work::default(),
                scratch: ff::av_frame_alloc(),
                copy: ff::av_frame_alloc(),
                packet: ff::av_packet_alloc(),
                scaler: ptr::null_mut(),
                pool: Pool::default(),
                on_device: None,
                ended: false,
                method: "software".into(),
            };
            if (*st).start_time != ff::AV_NOPTS_VALUE {
                this.start = (*st).start_time as f64 * this.time_base;
            }
            this.tick = (this.time_base / 2.0).clamp(1e-6, LARGEST_TICK);
            let r = ff::avcodec_parameters_to_context(codec, par);
            if r < 0 {
                return Err(explain(path, "Reading the video settings", r));
            }
            (*codec).thread_count = 0;
            (*codec).thread_type = ff::FF_THREAD_FRAME | ff::FF_THREAD_SLICE;
            match (way, hardware) {
                (Way::Device(device), Some((_, format))) => this.use_device(device, format)?,
                (_, Some((_, format))) if !this.try_hardware(format) => {
                    // No GPU device after all: the default decoder, in software.
                    drop(this);
                    return VideoDecoder::open(path, stream, rotation, Decoding::Software);
                }
                _ => {}
            }
            let r = ff::avcodec_open2(codec, decoder, ptr::null_mut());
            if r < 0 {
                return Err(explain(path, "Opening the decoder", r));
            }
            this.keys = this.listed_key_frames();
            Ok(this)
        }
    }

    /// The key frames the container lists for this stream, without reading
    /// the file through. Call before anything is decoded: some containers
    /// only read their list when first asked to jump (until then they know
    /// the few key frames passed while the file was opened), so the file is
    /// made to jump to its start.
    fn listed_key_frames(&mut self) -> KeyFrames {
        let mut times = Vec::new();
        // SAFETY: the context is open and the stream index was checked when
        // it was opened; entries are read while nothing else uses the context.
        unsafe {
            let raw = self.input.raw();
            let st = *(*raw).streams.add(self.stream as usize);
            let first = if (*st).start_time == ff::AV_NOPTS_VALUE {
                0
            } else {
                (*st).start_time
            };
            ff::av_seek_frame(raw, self.stream, first, ff::AVSEEK_FLAG_BACKWARD);
            for i in 0..ff::avformat_index_get_entries_count(st) {
                let entry = ff::avformat_index_get_entry(st, i);
                if !entry.is_null() && (*entry).flags() & ff::AVINDEX_KEYFRAME != 0 {
                    times.push((*entry).timestamp as f64 * self.time_base - self.start);
                }
            }
            // The list may hold decode times: a frame is shown up to this
            // long after it is decoded (the stream's reordering, and one
            // frame more to be on the safe side).
            let rate = (*st).avg_frame_rate;
            let frame = if rate.num > 0 && rate.den > 0 {
                f64::from(rate.den) / f64::from(rate.num)
            } else {
                1.0 / 24.0
            };
            let reordered_by = f64::from((*(*st).codecpar).video_delay.max(0) + 1);
            KeyFrames::new(times, reordered_by * frame)
        }
    }

    /// The key frames the file's container lists for this stream (it may
    /// list none, or not all).
    pub fn key_frames(&self) -> &KeyFrames {
        &self.keys
    }

    /// Where the time of the last picture went.
    pub fn last_work(&self) -> Work {
        self.work
    }

    /// Sets the codec up to decode on the window's device, into `format`
    /// frames that stay on the graphics card.
    unsafe fn use_device(
        &mut self,
        device: &Arc<WindowDevice>,
        format: ff::AVPixelFormat,
    ) -> diag::Result<()> {
        let copier = Copier::new(device.device()).map_err(device_error)?;
        // SAFETY: called before avcodec_open2 on a fresh context; the new
        // reference to FFmpeg's device context is handed to the codec
        // context, which owns it from then on.
        unsafe {
            let reference = device.new_reference();
            if reference.is_null() {
                return Err(device_error("Memory ran out."));
            }
            (*self.codec).hw_device_ctx = reference;
            (*self.codec).opaque = format as i32 as isize as *mut std::ffi::c_void;
            // Only the device's own frames will do: anything else is an
            // error here, and the file is opened the usual way instead.
            (*self.codec).get_format = Some(pick_format_or_none);
            // One thread, as in the measurements: the graphics card does the work.
            (*self.codec).thread_count = 1;
        }
        self.hw_format = Some(format);
        self.on_device = Some((Arc::clone(device), copier));
        self.method = "hardware (vulkan, stays on the GPU)".into();
        Ok(())
    }

    /// Hands the codec a GPU device producing `format` frames. False when
    /// no device could be made.
    unsafe fn try_hardware(&mut self, format: ff::AVPixelFormat) -> bool {
        // SAFETY: called before avcodec_open2 on a fresh context; the device
        // reference is handed to the codec context, which owns it from then on.
        unsafe {
            let Some(kind) = hardware_kind() else { return false };
            let mut device = ptr::null_mut();
            if ff::av_hwdevice_ctx_create(&mut device, kind, ptr::null(), ptr::null_mut(), 0) < 0 {
                return false;
            }
            (*self.codec).hw_device_ctx = device;
            (*self.codec).opaque = format as i32 as isize as *mut std::ffi::c_void;
            (*self.codec).get_format = Some(pick_format);
            self.hw_format = Some(format);
            self.method = format!("hardware ({HARDWARE})");
            true
        }
    }

    /// The frame on screen at `seconds` from the start of the file, scaled
    /// to fit in `max_width × max_height` (never enlarged) and turned upright.
    pub fn frame_at(&mut self, seconds: f64, max_width: u32, max_height: u32) -> diag::Result<Frame> {
        self.hold(seconds, &mut || Newer::Nothing)?;
        self.held_as_frame(max_width, max_height)
    }

    /// [`VideoDecoder::frame_at`] for a request that may be overtaken: see
    /// [`VideoDecoder::picture_at_while`].
    pub fn frame_at_while(
        &mut self,
        seconds: f64,
        max_width: u32,
        max_height: u32,
        newer: &mut dyn FnMut() -> Newer,
    ) -> diag::Result<Reached<Frame>> {
        match self.hold(seconds, newer)? {
            Some(at) => self
                .held_as_frame(max_width, max_height)
                .map(|picture| Reached::Shown { picture, at }),
            None => Ok(Reached::Dropped),
        }
    }

    /// The frame on screen at `seconds`, as the decoder's own planes when
    /// its format allows (nothing is converted or turned: the GPU does both
    /// while drawing), else as [`VideoDecoder::frame_at`] gives it.
    ///
    /// A decoder on the window's device gives the picture as images on the
    /// graphics card, always at the file's size, or fails with the reason
    /// (a format the shader doesn't take, a problem on the card): open the
    /// file one [`Level`] down then.
    ///
    /// The planes come at the file's size, or at a half, a quarter or an
    /// eighth of it when that still fills `max_width × max_height` as
    /// sharply as the box can show.
    pub fn picture_at(&mut self, seconds: f64, max_width: u32, max_height: u32) -> diag::Result<Decoded> {
        self.hold(seconds, &mut || Newer::Nothing)?;
        self.held_as_picture(max_width, max_height)
    }

    /// [`VideoDecoder::picture_at`] for a request that may be overtaken
    /// while it is decoded. Getting to a moment can mean decoding a hundred
    /// frames; between frames `newer` is asked what is wanted by now:
    ///
    /// - the same, or a later moment this decode is on the way to: it is
    ///   finished, and its picture shown, first;
    /// - an earlier moment this decode has not passed yet: decoding stops
    ///   there, and that picture is given instead;
    /// - anything this decode can't serve (a moment behind it, one a key
    ///   frame away, another file): it is given up at once.
    ///
    /// So a jump that has been overtaken never holds up the next one, and
    /// decoding that is going the right way is never thrown away.
    pub fn picture_at_while(
        &mut self,
        seconds: f64,
        max_width: u32,
        max_height: u32,
        newer: &mut dyn FnMut() -> Newer,
    ) -> diag::Result<Reached<Decoded>> {
        match self.hold(seconds, newer)? {
            Some(at) => self
                .held_as_picture(max_width, max_height)
                .map(|picture| Reached::Shown { picture, at }),
            None => Ok(Reached::Dropped),
        }
    }

    /// The frame held, scaled and turned upright in memory.
    fn held_as_frame(&mut self, max_width: u32, max_height: u32) -> diag::Result<Frame> {
        let source = self.in_memory()?;
        // SAFETY: `source` was just returned by `in_memory`.
        unsafe { self.convert(source, max_width, max_height) }
    }

    /// The frame held, the way [`VideoDecoder::picture_at`] gives it.
    fn held_as_picture(&mut self, max_width: u32, max_height: u32) -> diag::Result<Decoded> {
        if let Some((device, copier)) = &mut self.on_device {
            let time = self.held_time.unwrap_or(0.0);
            // SAFETY: `held` holds a frame this decoder decoded on `device`,
            // and stays as it is until the next call on self.
            return unsafe { device.copy_out(self.held, copier, self.rotation, time) }.map(Decoded::Gpu);
        }
        let source = self.in_memory()?;
        // SAFETY: `source` was just returned by `in_memory`.
        unsafe {
            match self.planes(source, max_width, max_height) {
                Some(frame) => Ok(Decoded::Yuv(frame)),
                None => self.convert(source, max_width, max_height).map(Decoded::Rgba),
            }
        }
    }

    /// Whether getting to `target` from where the decoder is now is quicker
    /// by jumping than by decoding on.
    fn jumps_to(&self, target: f64) -> bool {
        // A frame stored a tick late is still the frame at `target`: not behind it.
        self.keys
            .jump(self.held_time, self.ahead_time, target + self.tick, self.ended)
    }

    /// Decodes (jumping first when that is quicker) until `held` is the
    /// frame on screen at `seconds`, asking `newer` between frames what is
    /// wanted by now (see [`VideoDecoder::picture_at_while`]). Returns the
    /// moment `held` is the frame for, or `None` when the decode was given up.
    fn hold(&mut self, seconds: f64, newer: &mut dyn FnMut() -> Newer) -> diag::Result<Option<f64>> {
        let started = Instant::now();
        let mut target = seconds.max(0.0);
        self.work = Work::default();
        if self.jumps_to(target) {
            self.seek(target)?;
            self.work.jumped = true;
            self.work.jump = started.elapsed();
        }
        let reached = loop {
            if let Some(t) = self.ahead_time {
                // The frame on screen at `target` is the last one due by
                // then, give or take what its stored time may be off by.
                if t <= target + self.tick || self.held_time.is_none() {
                    // SAFETY: both frames are owned by self; `held` is cleared first.
                    unsafe {
                        ff::av_frame_unref(self.held);
                        ff::av_frame_move_ref(self.held, self.ahead);
                    }
                    self.held_time = self.ahead_time.take();
                    continue;
                }
                break Some(target);
            }
            // Another frame has to be decoded: is this still what is wanted?
            match newer() {
                Newer::Nothing => {}
                Newer::Elsewhere => break None,
                Newer::At { moment, give_up } => {
                    let moment = moment.max(0.0);
                    if !self.jumps_to(moment) {
                        // Reachable by decoding on. Earlier than the target:
                        // stop there. Later: the target is on the way.
                        target = target.min(moment);
                    } else if give_up {
                        break None;
                    }
                }
            }
            match self.decode_next()? {
                Some(t) => {
                    self.ahead_time = Some(t);
                    self.work.frames += 1;
                    self.work.first_frame.get_or_insert_with(|| started.elapsed());
                }
                None => break Some(target),
            }
        };
        self.work.decoding = started.elapsed();
        if reached.is_some() && self.held_time.is_none() {
            return Err(AppError::new("The video can't be shown")
                .why("No picture could be decoded at this point of the file.")
                .fix("The file may be damaged. Check that it plays in a media player."));
        }
        Ok(reached)
    }

    fn seek(&mut self, target: f64) -> diag::Result<()> {
        let ts = ((target + self.start) / self.time_base) as i64;
        // SAFETY: the context and codec are valid; frames are unreferenced, not freed.
        unsafe {
            let r = ff::av_seek_frame(self.input.raw(), self.stream, ts, ff::AVSEEK_FLAG_BACKWARD);
            if r < 0 && target > 0.0 {
                // Some files only seek to the start.
                ff::av_seek_frame(self.input.raw(), self.stream, 0, ff::AVSEEK_FLAG_BACKWARD);
            }
            ff::avcodec_flush_buffers(self.codec);
            ff::av_frame_unref(self.held);
            ff::av_frame_unref(self.ahead);
        }
        self.held_time = None;
        self.ahead_time = None;
        self.ended = false;
        Ok(())
    }

    /// Decodes the next frame into `ahead`; returns its time, or `None` at the end.
    fn decode_next(&mut self) -> diag::Result<Option<f64>> {
        // SAFETY: all pointers are owned by self and valid; packets are
        // unreferenced after sending; FFmpeg's send/receive contract is followed.
        unsafe {
            loop {
                let r = ff::avcodec_receive_frame(self.codec, self.ahead);
                if r == 0 {
                    let ts = (*self.ahead).best_effort_timestamp;
                    let t = if ts == ff::AV_NOPTS_VALUE {
                        self.held_time.map_or(0.0, |h| h + 1.0 / 60.0)
                    } else {
                        ts as f64 * self.time_base - self.start
                    };
                    return Ok(Some(t));
                }
                if r == ff::AVERROR_EOF {
                    self.ended = true;
                    return Ok(None);
                }
                if r != ff::AVERROR(ff::EAGAIN) {
                    return Err(AppError::new("The video can't be shown")
                        .why(format!("Decoding failed ({}).", error_text(r)))
                        .fix(
                            "The file may be damaged at this point. Check that it plays in a media player.",
                        ));
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
                        if r < 0 && r != ff::AVERROR(ff::EAGAIN) && r != ff::AVERROR_EOF {
                            // A broken packet: skip it and carry on, as players do.
                            continue;
                        }
                        break;
                    }
                    ff::av_packet_unref(self.packet);
                }
            }
        }
    }

    /// `held` in ordinary memory: itself, or copied back from the GPU into
    /// `copy`. The frame returned stays as it is until the next call on self.
    fn in_memory(&mut self) -> diag::Result<*mut ff::AVFrame> {
        // SAFETY: `held` holds a decoded frame and `copy` is a frame owned by
        // self; FFmpeg allocates the copy's buffers.
        unsafe {
            if !self.hw_format.is_some_and(|f| (*self.held).format == f as i32) {
                return Ok(self.held);
            }
            ff::av_frame_unref(self.copy);
            let r = ff::av_hwframe_transfer_data(self.copy, self.held, 0);
            if r < 0 {
                return Err(AppError::new("The video can't be shown")
                    .why(format!(
                        "Copying the picture from the GPU failed ({}).",
                        error_text(r)
                    ))
                    .fix("Update the graphics driver. If it keeps happening, report it."));
            }
            Ok(self.copy)
        }
    }

    /// How `held` stores colour. The copy from the GPU carries only pixels,
    /// so this is read from the decoded frame itself. `full_range_format`
    /// is whether the pixels came in one of the old full-range formats.
    fn colour(&self, full_range_format: bool, width: u32, height: u32) -> Option<Colour> {
        // SAFETY: `held` holds a decoded frame.
        let (space, range) = unsafe { ((*self.held).colorspace, (*self.held).color_range) };
        Colour::of(space, range, full_range_format, width, height)
    }

    /// `held` as YUV planes, copied out once. `None` when its format isn't
    /// 8-bit 4:2:0 with a colour matrix the shader knows.
    ///
    /// # Safety
    /// `source` must be what `in_memory` returned for the frame now held.
    unsafe fn planes(&self, source: *mut ff::AVFrame, max_width: u32, max_height: u32) -> Option<YuvFrame> {
        let time = self.held_time.unwrap_or(0.0);
        // SAFETY: `source` is a decoded frame in ordinary memory (this
        // function's contract); each plane's pointer and line size are
        // checked before a slice is made over exactly the bytes FFmpeg
        // allocated for its rows; the slices end with this block.
        unsafe {
            use ff::AVPixelFormat::{AV_PIX_FMT_NV12, AV_PIX_FMT_YUV420P, AV_PIX_FMT_YUVJ420P};
            let format = (*source).format;
            let interleaved = format == AV_PIX_FMT_NV12 as i32;
            if !interleaved && format != AV_PIX_FMT_YUV420P as i32 && format != AV_PIX_FMT_YUVJ420P as i32 {
                return None;
            }
            let width = usize::try_from((*source).width).ok()?;
            let height = usize::try_from((*source).height).ok()?;
            let full_range_format = format == AV_PIX_FMT_YUVJ420P as i32;
            let colour = self.colour(full_range_format, width as u32, height as u32)?;
            let (cw, ch) = (width.div_ceil(2), height.div_ceil(2));
            let plane = |index: usize, row_bytes: usize, rows: usize| -> Option<PlaneRef<'_>> {
                let data = (*source).data[index];
                let stride = usize::try_from((*source).linesize[index]).ok()?;
                if data.is_null() || rows == 0 || row_bytes == 0 || stride < row_bytes {
                    return None;
                }
                Some(PlaneRef {
                    bytes: std::slice::from_raw_parts(data, stride * (rows - 1) + row_bytes),
                    stride,
                    row_bytes,
                    rows,
                })
            };
            let source = Source {
                luma: plane(0, width, height)?,
                chroma: if interleaved {
                    [Some(plane(1, cw * 2, ch)?), None]
                } else {
                    [Some(plane(1, cw, ch)?), Some(plane(2, cw, ch)?)]
                },
                colour,
                rotation: self.rotation,
                time,
            };
            let upright = if self.rotation % 180 == 90 {
                (height as u32, width as u32)
            } else {
                (width as u32, height as u32)
            };
            Some(source.frame(step_for(upright, max_width, max_height), &self.pool))
        }
    }

    /// Scales `held` to RGBA within the size given, then turns it upright.
    ///
    /// # Safety
    /// `source` must be what `in_memory` returned for the frame now held.
    unsafe fn convert(
        &mut self,
        source: *mut ff::AVFrame,
        max_width: u32,
        max_height: u32,
    ) -> diag::Result<Frame> {
        let time = self.held_time.unwrap_or(0.0);
        // SAFETY: `source` is a decoded frame in ordinary memory (this
        // function's contract); the scaler is owned by self; the
        // destination buffer is sized for the output.
        unsafe {
            let (sw, sh) = ((*source).width, (*source).height);
            if sw <= 0 || sh <= 0 {
                return Err(AppError::new("The video can't be shown").why("A decoded picture had no size."));
            }
            // Fit the upright picture in the box, then scale the stored one to match.
            let turned = self.rotation % 180 == 90;
            let (uw, uh) = if turned {
                (sh as u32, sw as u32)
            } else {
                (sw as u32, sh as u32)
            };
            let scale = (f64::from(max_width.max(1)) / f64::from(uw))
                .min(f64::from(max_height.max(1)) / f64::from(uh))
                .min(1.0);
            let (fw, fh) = (
                ((f64::from(uw) * scale).round() as u32).max(2) & !1,
                ((f64::from(uh) * scale).round() as u32).max(2) & !1,
            );
            let (dw, dh) = if turned {
                (fh as i32, fw as i32)
            } else {
                (fw as i32, fh as i32)
            };
            let src_format: ff::AVPixelFormat =
                std::mem::transmute::<i32, ff::AVPixelFormat>((*source).format);
            self.scaler = ff::sws_getCachedContext(
                self.scaler,
                sw,
                sh,
                src_format,
                dw,
                dh,
                ff::AVPixelFormat::AV_PIX_FMT_RGBA,
                ff::SwsFlags::SWS_BILINEAR as i32,
                ptr::null_mut(),
                ptr::null_mut(),
                ptr::null(),
            );
            if self.scaler.is_null() {
                return Err(AppError::new("The video can't be shown")
                    .why("The picture's format can't be converted for display.")
                    .fix("Report it with the file attached."));
            }
            self.set_scaler_colour(sw as u32, sh as u32);
            let mut rgba = vec![0u8; dw as usize * dh as usize * 4];
            let dst = [
                rgba.as_mut_ptr(),
                ptr::null_mut(),
                ptr::null_mut(),
                ptr::null_mut(),
            ];
            let stride = [dw * 4, 0, 0, 0];
            ff::sws_scale(
                self.scaler,
                (*source).data.as_ptr() as *const *const u8,
                (*source).linesize.as_ptr(),
                0,
                sh,
                dst.as_ptr(),
                stride.as_ptr(),
            );
            let (width, height, rgba) = turn(rgba, dw as u32, dh as u32, self.rotation);
            Ok(Frame {
                width,
                height,
                rgba,
                time,
            })
        }
    }

    /// Tells the scaler which colour matrix and range `held` uses, so the
    /// RGBA path shows the same colours as the planes path. Left alone, the
    /// scaler reads everything as BT.601 and only knows full range by the
    /// old format names.
    ///
    /// # Safety
    /// The scaler must have been made, and a decoded frame be held.
    unsafe fn set_scaler_colour(&mut self, width: u32, height: u32) {
        // SAFETY: the scaler was just made; `held` holds a decoded frame; the
        // coefficient tables are static.
        unsafe {
            let held = &*self.held;
            let space = match held.colorspace {
                // The same guess as the planes path when the file doesn't say.
                ff::AVColorSpace::AVCOL_SPC_UNSPECIFIED if is_high_definition(width, height) => {
                    ff::SWS_CS_ITU709
                }
                ff::AVColorSpace::AVCOL_SPC_UNSPECIFIED => ff::SWS_CS_ITU601,
                // FFmpeg numbers the scaler's tables as it numbers colour spaces.
                said => said as i32,
            };
            let (mut inverse, mut table) = (ptr::null_mut(), ptr::null_mut());
            let (mut source_range, mut target_range, mut brightness, mut contrast, mut saturation) =
                (0, 0, 0, 0, 0);
            // Not every format pair has colour settings (RGB sources don't).
            if ff::sws_getColorspaceDetails(
                self.scaler,
                &mut inverse,
                &mut source_range,
                &mut table,
                &mut target_range,
                &mut brightness,
                &mut contrast,
                &mut saturation,
            ) < 0
            {
                return;
            }
            if held.color_range == ff::AVColorRange::AVCOL_RANGE_JPEG {
                source_range = 1;
            }
            ff::sws_setColorspaceDetails(
                self.scaler,
                ff::sws_getCoefficients(space),
                source_range,
                table,
                target_range,
                brightness,
                contrast,
                saturation,
            );
        }
    }
}

/// The usual guess for a file that doesn't say which colour matrix it uses:
/// high definition sizes are BT.709, smaller ones BT.601.
pub(crate) fn is_high_definition(width: u32, height: u32) -> bool {
    width >= 1280 || height > 576
}

/// Turns an RGBA picture clockwise by 0, 90, 180 or 270 degrees.
pub(crate) fn turn(rgba: Vec<u8>, w: u32, h: u32, clockwise: u16) -> (u32, u32, Vec<u8>) {
    let (w, h) = (w as usize, h as usize);
    let px = |x: usize, y: usize| &rgba[(y * w + x) * 4..(y * w + x) * 4 + 4];
    match clockwise % 360 {
        90 => {
            let mut out = Vec::with_capacity(rgba.len());
            for y in 0..w {
                for x in 0..h {
                    out.extend_from_slice(px(y, h - 1 - x));
                }
            }
            (h as u32, w as u32, out)
        }
        180 => {
            let mut out = Vec::with_capacity(rgba.len());
            for y in 0..h {
                for x in 0..w {
                    out.extend_from_slice(px(w - 1 - x, h - 1 - y));
                }
            }
            (w as u32, h as u32, out)
        }
        270 => {
            let mut out = Vec::with_capacity(rgba.len());
            for y in 0..w {
                for x in 0..h {
                    out.extend_from_slice(px(w - 1 - y, x));
                }
            }
            (h as u32, w as u32, out)
        }
        _ => (w as u32, h as u32, rgba),
    }
}

impl Drop for VideoDecoder {
    fn drop(&mut self) {
        // SAFETY: each pointer was allocated by FFmpeg in `open` and is freed once.
        unsafe {
            ff::sws_freeContext(self.scaler);
            ff::av_frame_free(&mut self.held);
            ff::av_frame_free(&mut self.ahead);
            ff::av_frame_free(&mut self.scratch);
            ff::av_frame_free(&mut self.copy);
            ff::av_packet_free(&mut self.packet);
            ff::avcodec_free_context(&mut self.codec);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_8_bit_pictures_with_colour_at_half_size_are_decoded_on_the_device() {
        use ff::AVPixelFormat::*;
        for taken in [AV_PIX_FMT_YUV420P, AV_PIX_FMT_YUVJ420P, AV_PIX_FMT_NONE] {
            assert!(shader_takes(taken as i32), "{taken:?}");
        }
        for other in [
            AV_PIX_FMT_YUV420P10LE,
            AV_PIX_FMT_YUV422P,
            AV_PIX_FMT_YUV444P,
            AV_PIX_FMT_RGB24,
        ] {
            assert!(!shader_takes(other as i32), "{other:?}");
        }
    }

    #[test]
    fn av1_on_the_gpu_uses_the_decoder_that_supports_it() {
        let kinds = [
            ff::AVHWDeviceType::AV_HWDEVICE_TYPE_VAAPI,
            ff::AVHWDeviceType::AV_HWDEVICE_TYPE_D3D11VA,
            ff::AVHWDeviceType::AV_HWDEVICE_TYPE_VULKAN,
        ];
        // Whichever GPU interfaces this FFmpeg build has, the AV1 decoder
        // offered for them is FFmpeg's own, never the software-only default.
        let found: Vec<String> = kinds
            .iter()
            .filter_map(|k| decoder_for(ff::AVCodecID::AV_CODEC_ID_AV1, *k))
            .map(|(c, _)| {
                unsafe { std::ffi::CStr::from_ptr((*c).name) }
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        assert!(!found.is_empty(), "this FFmpeg build has a GPU AV1 decoder");
        assert!(found.iter().all(|n| n == "av1"), "{found:?}");
        let default = unsafe {
            std::ffi::CStr::from_ptr((*ff::avcodec_find_decoder(ff::AVCodecID::AV_CODEC_ID_AV1)).name)
        };
        assert_eq!(
            default.to_str().unwrap(),
            "libdav1d",
            "the default stays software"
        );
    }

    #[test]
    fn turning_moves_pixels_clockwise() {
        // 2×1: red, green.
        let img = vec![255, 0, 0, 255, 0, 255, 0, 255];
        let (w, h, out) = turn(img.clone(), 2, 1, 90);
        assert_eq!((w, h), (1, 2));
        assert_eq!(
            &out[..4],
            &[255, 0, 0, 255],
            "red ends on top after a clockwise quarter turn"
        );
        let (w, h, out) = turn(img.clone(), 2, 1, 180);
        assert_eq!((w, h), (2, 1));
        assert_eq!(&out[..4], &[0, 255, 0, 255]);
        let (_, _, out) = turn(img, 2, 1, 270);
        assert_eq!(
            &out[..4],
            &[0, 255, 0, 255],
            "green ends on top after a counter-clockwise turn"
        );
    }
}
