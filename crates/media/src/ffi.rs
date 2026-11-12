//! The raw FFmpeg calls, each wrapped so the rest of the crate never touches
//! a pointer. Every `unsafe` block says why it is sound.

use ffmpeg_sys_next as ff;
use std::ffi::{CStr, CString, c_char};
use std::path::Path;
use std::ptr;
use std::sync::Once;

/// FFmpeg's own messages go to its log, which is noisy; we explain errors ourselves.
fn quiet() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        // SAFETY: sets a global integer; FFmpeg allows this at any time.
        unsafe { ff::av_log_set_level(ff::AV_LOG_FATAL) }
    });
}

/// FFmpeg's description of an error code.
pub fn error_text(code: i32) -> String {
    let mut buf = [0 as c_char; 256];
    // SAFETY: the buffer is writable and its length is passed; FFmpeg always
    // terminates the string within it.
    unsafe {
        ff::av_strerror(code, buf.as_mut_ptr(), buf.len());
        CStr::from_ptr(buf.as_ptr()).to_string_lossy().into_owned()
    }
}

/// Copies an optional C string owned by FFmpeg.
///
/// # Safety
/// `p` must be null or point to a NUL-terminated string that stays valid during the call.
unsafe fn owned(p: *const c_char) -> Option<String> {
    if p.is_null() {
        None
    } else {
        // SAFETY: non-null and NUL-terminated per this function's contract.
        Some(unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned())
    }
}

/// Why opening failed, in FFmpeg's words, plus whether the file is simply
/// not media (as opposed to unreadable).
#[derive(Debug)]
pub struct OpenError {
    pub text: String,
    pub not_media: bool,
}

/// An opened media file with its stream information read.
pub struct Input {
    ctx: *mut ff::AVFormatContext,
}

impl Input {
    pub fn open(path: &Path) -> Result<Input, OpenError> {
        quiet();
        // FFmpeg takes UTF-8 paths on every system.
        let Some(text) = path.to_str() else {
            return Err(OpenError {
                text: "the file's name has characters that can't be passed on".into(),
                not_media: false,
            });
        };
        let c_path = CString::new(text).map_err(|_| OpenError {
            text: "the file's name has a zero byte".into(),
            not_media: false,
        })?;
        let mut ctx = ptr::null_mut();
        // SAFETY: `ctx` starts null so FFmpeg allocates it; the path is a valid
        // C string for the duration of the call; no format or options are forced.
        let r = unsafe { ff::avformat_open_input(&mut ctx, c_path.as_ptr(), ptr::null(), ptr::null_mut()) };
        if r < 0 {
            return Err(OpenError {
                text: error_text(r),
                not_media: r == ff::AVERROR_INVALIDDATA,
            });
        }
        let input = Input { ctx };
        // SAFETY: `ctx` was just opened successfully and is owned by `input`.
        let r = unsafe { ff::avformat_find_stream_info(input.ctx, ptr::null_mut()) };
        if r < 0 {
            return Err(OpenError {
                text: error_text(r),
                not_media: true,
            });
        }
        Ok(input)
    }

    /// The raw context, for the decoder in this crate.
    pub(crate) fn raw(&self) -> *mut ff::AVFormatContext {
        self.ctx
    }

    fn ctx(&self) -> &ff::AVFormatContext {
        // SAFETY: `ctx` is non-null and valid for as long as `self` lives.
        unsafe { &*self.ctx }
    }

    /// The container's short names, such as "matroska,webm".
    pub fn container(&self) -> String {
        let format = self.ctx().iformat;
        if format.is_null() {
            return String::new();
        }
        // SAFETY: `iformat` points to FFmpeg's static format description.
        unsafe { owned((*format).name) }.unwrap_or_default()
    }

    /// Length in microseconds, if the container states it.
    pub fn duration_us(&self) -> Option<i64> {
        let d = self.ctx().duration;
        (d > 0 && d != ff::AV_NOPTS_VALUE).then_some(d)
    }

    /// Presentation times (seconds) of the packets of `stream` from about
    /// `from` seconds on, for up to `span` seconds of file. Reads packets only;
    /// nothing is decoded. Empty when the file can't be read there.
    pub fn packet_times(&self, stream: u32, from: f64, span: f64) -> Vec<f64> {
        let ctx = self.ctx();
        if stream >= ctx.nb_streams {
            return Vec::new();
        }
        // SAFETY: the index was checked against `nb_streams`; stream pointers
        // stay valid while the input is open.
        let tb = unsafe { (**ctx.streams.add(stream as usize)).time_base };
        if tb.num <= 0 || tb.den <= 0 {
            return Vec::new();
        }
        let tb = f64::from(tb.num) / f64::from(tb.den);
        let mut times = Vec::new();
        // SAFETY: the packet is allocated here, filled by av_read_frame,
        // unreferenced after each use and freed before returning; the format
        // context is valid and only this thread uses it.
        unsafe {
            let target = (from / tb) as i64;
            let _ = ff::av_seek_frame(self.ctx, stream as i32, target, ff::AVSEEK_FLAG_BACKWARD);
            let mut packet = ff::av_packet_alloc();
            if packet.is_null() {
                return times;
            }
            let mut first: Option<f64> = None;
            while ff::av_read_frame(self.ctx, packet) >= 0 {
                let p = &*packet;
                if p.stream_index as u32 == stream && p.pts != ff::AV_NOPTS_VALUE {
                    let t = p.pts as f64 * tb;
                    let start = *first.get_or_insert(t);
                    if t - start > span {
                        ff::av_packet_unref(packet);
                        break;
                    }
                    times.push(t);
                }
                ff::av_packet_unref(packet);
            }
            ff::av_packet_free(&mut packet);
        }
        times
    }

    pub fn streams(&self) -> Vec<Stream<'_>> {
        let ctx = self.ctx();
        (0..ctx.nb_streams as usize)
            .filter_map(|i| {
                // SAFETY: `streams` holds `nb_streams` valid stream pointers.
                let s = unsafe { *ctx.streams.add(i) };
                // SAFETY: each stream pointer is valid while the input is open.
                (!s.is_null()).then(|| Stream { s: unsafe { &*s } })
            })
            .collect()
    }
}

impl Drop for Input {
    fn drop(&mut self) {
        // SAFETY: `ctx` came from a successful avformat_open_input and is closed once.
        unsafe { ff::avformat_close_input(&mut self.ctx) }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamKind {
    Video,
    Audio,
    Other,
}

/// What this build of FFmpeg can do with a stream's codec.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodecSupport {
    Decodable,
    /// FFmpeg knows the codec but this build has no decoder for it.
    NoDecoder,
    /// FFmpeg doesn't know the codec at all (newer, or not a real one).
    Unknown,
}

/// One stream of an open input. Borrowing the input keeps it alive.
pub struct Stream<'a> {
    s: &'a ff::AVStream,
}

impl Stream<'_> {
    fn par(&self) -> Option<&ff::AVCodecParameters> {
        // SAFETY: `codecpar` is null or valid while the stream lives.
        unsafe { self.s.codecpar.as_ref() }
    }

    pub fn index(&self) -> u32 {
        self.s.index.max(0) as u32
    }

    pub fn kind(&self) -> StreamKind {
        match self.par().map(|p| p.codec_type) {
            Some(ff::AVMediaType::AVMEDIA_TYPE_VIDEO) => StreamKind::Video,
            Some(ff::AVMediaType::AVMEDIA_TYPE_AUDIO) => StreamKind::Audio,
            _ => StreamKind::Other,
        }
    }

    /// Cover art and similar pictures stored as a one-frame "video" stream.
    pub fn is_attached_picture(&self) -> bool {
        self.s.disposition & ff::AV_DISPOSITION_ATTACHED_PIC != 0
    }

    pub fn codec(&self) -> String {
        let Some(par) = self.par() else {
            return String::new();
        };
        // SAFETY: avcodec_get_name returns a static string for any id.
        unsafe { owned(ff::avcodec_get_name(par.codec_id)) }.unwrap_or_default()
    }

    /// Whether this build of FFmpeg can decode the stream.
    pub fn codec_support(&self) -> CodecSupport {
        let Some(par) = self.par() else {
            return CodecSupport::Unknown;
        };
        if par.codec_id == ff::AVCodecID::AV_CODEC_ID_NONE {
            return CodecSupport::Unknown;
        }
        // SAFETY: looks the id up in FFmpeg's static table of decoders; any
        // id is allowed, and the result is only compared with null.
        let decoder = unsafe { ff::avcodec_find_decoder(par.codec_id) };
        if decoder.is_null() {
            CodecSupport::NoDecoder
        } else {
            CodecSupport::Decodable
        }
    }

    pub fn size(&self) -> (u32, u32) {
        self.par()
            .map_or((0, 0), |p| (p.width.max(0) as u32, p.height.max(0) as u32))
    }

    /// Average frame rate, else the base rate, as a fraction.
    pub fn frame_rate(&self) -> Option<(u32, u32)> {
        [self.s.avg_frame_rate, self.s.r_frame_rate]
            .into_iter()
            .find(|r| r.num > 0 && r.den > 0)
            .map(|r| (r.num as u32, r.den as u32))
    }

    /// Length in seconds as a fraction (ticks, ticks per second), if stated.
    pub fn duration(&self) -> Option<(i64, i64)> {
        let tb = self.s.time_base;
        (self.s.duration > 0 && self.s.duration != ff::AV_NOPTS_VALUE && tb.den > 0)
            .then(|| (self.s.duration * i64::from(tb.num), i64::from(tb.den)))
    }

    pub fn channels(&self) -> u16 {
        self.par()
            .map_or(0, |p| p.ch_layout.nb_channels.clamp(0, u16::MAX as i32) as u16)
    }

    pub fn sample_rate(&self) -> u32 {
        self.par().map_or(0, |p| p.sample_rate.max(0) as u32)
    }

    /// A metadata value such as "title", "language" or "handler_name".
    pub fn tag(&self, key: &str) -> Option<String> {
        let key = CString::new(key).ok()?;
        // SAFETY: the dictionary is owned by the stream; the key is a valid C
        // string; the returned entry stays valid until the dictionary changes,
        // and its value is copied straight away.
        unsafe {
            let entry = ff::av_dict_get(self.s.metadata, key.as_ptr(), ptr::null(), 0);
            if entry.is_null() {
                None
            } else {
                owned((*entry).value).filter(|v| !v.trim().is_empty())
            }
        }
    }

    /// The display matrix (3×3, 16.16 and 2.30 fixed point), if the file has one.
    pub fn display_matrix(&self) -> Option<[i32; 9]> {
        let par = self.par()?;
        // SAFETY: `coded_side_data` holds `nb_coded_side_data` entries; the
        // matrix entry, when present, is at least 9 × 4 bytes as FFmpeg
        // guarantees for AV_PKT_DATA_DISPLAYMATRIX; it is copied out unaligned.
        unsafe {
            let sd = ff::av_packet_side_data_get(
                par.coded_side_data,
                par.nb_coded_side_data,
                ff::AVPacketSideDataType::AV_PKT_DATA_DISPLAYMATRIX,
            );
            if sd.is_null() || (*sd).size < 36 || (*sd).data.is_null() {
                return None;
            }
            Some(ptr::read_unaligned((*sd).data as *const [i32; 9]))
        }
    }
}
