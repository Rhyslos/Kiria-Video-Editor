//! Writing a proxy: a small copy of a recording that is cheap to jump
//! around in (`docs/research/caching.md`, approach C).
//!
//! The picture stream is decoded from start to end and written again as
//! H.264 with the shorter side 540 pixels, a key frame every 8 frames and no
//! frames that depend on later ones, so any frame is at most 7 small frames
//! from a key frame. Every frame keeps its own timestamp from the original,
//! in the original's time base, counted from the original's zero: the proxy
//! has exactly the original's frame times, also with a variable frame rate.
//! There is no sound (the mixer always reads the original), and the picture
//! is stored the way the original stores it, sideways or not: whoever shows
//! it turns it as the original is turned.
//!
//! The encoder is the first of [`ENCODERS`] that opens and keeps the frame
//! times: the graphics cards' own, then the CPU's. A proxy is decoded in
//! software on one thread, whatever made it.

use crate::decode::{Decoding, PassedFrame, VideoDecoder, is_high_definition};
use crate::ffi::error_text;
use diag::AppError;
use ffmpeg_sys_next as ff;
use std::collections::VecDeque;
use std::ffi::CString;
use std::path::Path;
use std::ptr;

/// The shorter side of a proxy's picture, in pixels (never more than the
/// original's).
pub const SHORT_SIDE: u32 = 540;
/// A key frame every this many frames.
pub const KEY_EVERY: i32 = 8;
/// The H.264 encoders tried, in order: the graphics cards' own, then the
/// CPU's. (FFmpeg's names for them.)
pub const ENCODERS: [&str; 4] = ["h264_amf", "h264_nvenc", "h264_qsv", "libx264"];
/// The one that needs no graphics card.
pub const SOFTWARE_ENCODER: &str = "libx264";

/// What was written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Written {
    /// Frames in the proxy.
    pub frames: u64,
    /// The encoder that made it (FFmpeg's name).
    pub encoder: String,
    /// Its picture's size as stored.
    pub size: (u32, u32),
}

/// What a proxy build is told and asks while it runs.
pub struct Steps<'a> {
    /// Asked between frames: stop and leave nothing behind.
    pub cancelled: &'a (dyn Fn() -> bool + Sync),
    /// Told the share done (0 to 1) now and then.
    pub progress: &'a (dyn Fn(f32) + Sync),
    /// Called between frames; returns when the build may go on (it waits
    /// there while the timeline plays). Several stretches of one file may
    /// be built side by side, so all three are called from several threads.
    pub pause: &'a (dyn Fn() + Sync),
}

impl std::fmt::Debug for Steps<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Steps")
    }
}

/// The size of a proxy for a picture stored `coded` big: the shorter side
/// [`SHORT_SIDE`] (or the original's, if smaller), the other in proportion,
/// both even.
pub fn size_for(coded: (u32, u32)) -> (u32, u32) {
    let (w, h) = (coded.0.max(2), coded.1.max(2));
    let short = w.min(h);
    let scale = f64::from(SHORT_SIDE.min(short)) / f64::from(short);
    let even = |side: u32| (((f64::from(side) * scale).round() as u32).max(2) + 1) & !1;
    (even(w), even(h))
}

fn cannot(why: impl Into<String>) -> AppError {
    AppError::new("The proxy can't be made")
        .why(why)
        .fix("Check that the disk has room. The original is used meanwhile; the proxy can be built again from the file's right-click menu.")
}

fn failed(what: &str, code: i32) -> AppError {
    cannot(format!("{what} failed: {}.", error_text(code)))
}

/// An encoder that is built in but doesn't open here (a graphics card's
/// encoder without that card or its driver): the next one is tried.
fn not_usable(name: &str, code: i32) -> AppError {
    AppError::new("This encoder can't be used here")
        .why(format!(
            "Opening the encoder {name} failed: {}.",
            error_text(code)
        ))
        .fix("Nothing to do: the next encoder is tried, ending with the processor's own.")
}

/// The error for a build that was stopped (cancelled) before the end.
pub fn stopped() -> AppError {
    AppError::new("The proxy was not finished").why("Its build was stopped.")
}

/// Writes the proxy of picture stream `stream` of `original` to `out` (an
/// MP4, whatever its name ends with), decoding the original `decoding`'s
/// way. `length` is the stream's length in seconds, for progress. With
/// `software` only the CPU's encoder is tried. A file left by a failed or
/// stopped build is removed.
///
/// With `workers` over one, a long file is built in that many stretches
/// side by side ([`stretches`]: each from a key frame, each decoded,
/// shrunk and encoded on threads of its own) and the stretches joined
/// into one file: the same frames and times as one pass, in a fraction of
/// the time on a processor with many cores.
#[allow(clippy::too_many_arguments)]
pub fn write(
    original: &Path,
    stream: u32,
    length: f64,
    out: &Path,
    decoding: Decoding,
    software: bool,
    workers: usize,
    steps: &Steps<'_>,
) -> diag::Result<Written> {
    let mut last = cannot("This build has no H.264 encoder.");
    for name in ENCODERS {
        if software && name != SOFTWARE_ENCODER {
            continue;
        }
        if !encoder_present(name) {
            continue;
        }
        let built = if workers > 1 {
            let split = Split {
                workers,
                shortest: SHORTEST_STRETCH,
            };
            write_split(original, stream, length, out, decoding, name, split, steps).or_else(|err| {
                if (steps.cancelled)() {
                    return Err(err);
                }
                // In one pass, as before: slower, but known to work.
                tracing::info!(
                    "a proxy in stretches did not work, building it in one pass: {}",
                    err.report()
                );
                let _ = std::fs::remove_file(out);
                write_with(original, stream, length, out, decoding, name, steps)
            })
        } else {
            write_with(original, stream, length, out, decoding, name, steps)
        };
        match built {
            Ok(written) => return Ok(written),
            Err(err) => {
                let _ = std::fs::remove_file(out);
                if (steps.cancelled)() {
                    return Err(stopped());
                }
                tracing::info!(
                    "a proxy with {name} did not work, trying the next encoder: {}",
                    err.report()
                );
                last = err;
            }
        }
    }
    Err(last)
}

fn encoder_present(name: &str) -> bool {
    let Ok(c_name) = CString::new(name) else {
        return false;
    };
    // SAFETY: a valid C string; only compared with null.
    !unsafe { ff::avcodec_find_encoder_by_name(c_name.as_ptr()) }.is_null()
}

fn write_with(
    original: &Path,
    stream: u32,
    length: f64,
    out: &Path,
    decoding: Decoding,
    encoder: &str,
    steps: &Steps<'_>,
) -> diag::Result<Written> {
    let mut decoder = VideoDecoder::open(original, stream, 0, decoding)?;
    let time_base = decoder.time_base_rational();
    let start = decoder.start_timestamp();
    let coded = decoder.coded_size();
    let size = size_for(coded);
    let seconds = f64::from(time_base.num) / f64::from(time_base.den.max(1));
    let mut writer: Option<Encoder> = None;
    let mut last_told = -1.0f32;
    decoder.each_frame(&mut |passed: &PassedFrame| {
        (steps.pause)();
        if (steps.cancelled)() {
            return Ok(false);
        }
        let writer = match &mut writer {
            Some(writer) => writer,
            None => writer.insert(Encoder::open(out, encoder, time_base, size, coded, passed.frame)?),
        };
        let timestamp = passed.timestamp - start;
        writer.add(passed.frame, timestamp)?;
        let done = ((timestamp as f64 * seconds) / length.max(0.001)).clamp(0.0, 1.0) as f32;
        if done - last_told >= 0.002 {
            (steps.progress)(done);
            last_told = done;
        }
        Ok(true)
    })?;
    if (steps.cancelled)() {
        return Err(stopped());
    }
    let writer = writer.ok_or_else(|| cannot("The original has no picture that could be decoded."))?;
    let frames = writer.frames;
    writer.finish()?;
    (steps.progress)(1.0);
    Ok(Written {
        frames,
        encoder: encoder.to_owned(),
        size,
    })
}

/// The shortest stretch worth building on its own (seconds): opening a
/// decoder and an encoder for less costs more than it saves.
const SHORTEST_STRETCH: f64 = 20.0;

/// Where the stretches after the first start, for a stream `length`
/// seconds long built by up to `workers` side by side: listed key frames
/// (`keys`, ascending) near equal shares of the length, each stretch at
/// least `shortest` seconds long ([`SHORTEST_STRETCH`] in a build). Empty:
/// build it in one pass.
pub fn stretches(keys: &[f64], length: f64, workers: usize, shortest: f64) -> Vec<f64> {
    let workers = workers.min((length / shortest.max(0.001)) as usize).max(1);
    let mut starts: Vec<f64> = Vec::new();
    for j in 1..workers {
        let aim = length * j as f64 / workers as f64;
        let Some(key) = keys.iter().copied().find(|k| *k >= aim) else {
            break;
        };
        let after = starts.last().copied().unwrap_or(0.0);
        if key - after >= shortest && length - key >= shortest {
            starts.push(key);
        }
    }
    starts
}

/// How a build is split ([`stretches`]).
#[derive(Debug, Clone, Copy)]
struct Split {
    workers: usize,
    shortest: f64,
}

/// [`write_with`] in stretches side by side, joined at the end. Falls
/// back to one pass where the file lists too few key frames.
#[allow(clippy::too_many_arguments)]
fn write_split(
    original: &Path,
    stream: u32,
    length: f64,
    out: &Path,
    decoding: Decoding,
    encoder: &str,
    split: Split,
    steps: &Steps<'_>,
) -> diag::Result<Written> {
    let keys = VideoDecoder::open_for_pass(original, stream, decoding, 1)?.listed_key_times();
    let starts = stretches(&keys, length, split.workers, split.shortest);
    if starts.is_empty() {
        return write_with(original, stream, length, out, decoding, encoder, steps);
    }
    let count = starts.len() + 1;
    let bounds: Vec<(Option<f64>, Option<f64>)> = (0..count)
        .map(|i| (i.checked_sub(1).map(|j| starts[j]), starts.get(i).copied()))
        .collect();
    let cores = std::thread::available_parallelism().map_or(2, |n| n.get());
    let threads = i32::try_from((cores / count).max(2)).unwrap_or(2);
    let parts: Vec<std::path::PathBuf> = (0..count)
        .map(|i| out.with_extension(format!("part{i}")))
        .collect();
    // Each stretch's share done, as f32 bits; told together.
    let shares: Vec<std::sync::atomic::AtomicU32> =
        (0..count).map(|_| std::sync::atomic::AtomicU32::new(0)).collect();
    let tell = || {
        let done: f32 = bounds
            .iter()
            .zip(&shares)
            .map(|((from, until), share)| {
                let span = until.unwrap_or(length) - from.unwrap_or(0.0);
                f32::from_bits(share.load(std::sync::atomic::Ordering::Relaxed))
                    * (span / length.max(0.001)) as f32
            })
            .sum();
        (steps.progress)(done.clamp(0.0, 1.0));
    };
    let built: Vec<diag::Result<Option<Written>>> = std::thread::scope(|scope| {
        let handles: Vec<_> = bounds
            .iter()
            .zip(&parts)
            .zip(&shares)
            .map(|((range, part), share)| {
                let tell = &tell;
                scope.spawn(move || {
                    let mut decoder = VideoDecoder::open_for_pass(original, stream, decoding, threads)?;
                    let span = range.1.unwrap_or(length) - range.0.unwrap_or(0.0);
                    let started = range.0.unwrap_or(0.0);
                    let progress = |seconds: f64| {
                        let done = ((seconds - started) / span.max(0.001)).clamp(0.0, 1.0) as f32;
                        share.store(done.to_bits(), std::sync::atomic::Ordering::Relaxed);
                        tell();
                    };
                    write_stretch(&mut decoder, *range, part, encoder, steps, &progress)
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|h| {
                h.join()
                    .unwrap_or_else(|_| Err(cannot("A part of the build stopped unexpectedly.")))
            })
            .collect()
    });
    let mut written = Vec::new();
    let mut failure = None;
    for (result, part) in built.into_iter().zip(&parts) {
        match result {
            Ok(Some(w)) => written.push((part.clone(), w)),
            Ok(None) => {}
            Err(err) => {
                failure.get_or_insert(err);
            }
        }
    }
    if (steps.cancelled)() {
        failure = Some(stopped());
    }
    let joined = match failure {
        Some(err) => Err(err),
        None => join(&written.iter().map(|(p, _)| p.clone()).collect::<Vec<_>>(), out),
    };
    for part in &parts {
        let _ = std::fs::remove_file(part);
    }
    joined?;
    let first = written
        .first()
        .ok_or_else(|| cannot("The original has no picture that could be decoded."))?;
    (steps.progress)(1.0);
    Ok(Written {
        frames: written.iter().map(|(_, w)| w.frames).sum(),
        encoder: encoder.to_owned(),
        size: first.1.size,
    })
}

/// One stretch of [`write_split`] into its own file. `None`: the stretch
/// held no frame.
fn write_stretch(
    decoder: &mut VideoDecoder,
    (from, until): (Option<f64>, Option<f64>),
    out: &Path,
    encoder: &str,
    steps: &Steps<'_>,
    progress: &dyn Fn(f64),
) -> diag::Result<Option<Written>> {
    let time_base = decoder.time_base_rational();
    let start = decoder.start_timestamp();
    let coded = decoder.coded_size();
    let size = size_for(coded);
    let seconds = f64::from(time_base.num) / f64::from(time_base.den.max(1));
    let mut writer: Option<Encoder> = None;
    let mut told = 0u32;
    decoder.each_frame_in(from, until, &mut |passed: &PassedFrame| {
        (steps.pause)();
        if (steps.cancelled)() {
            return Ok(false);
        }
        let writer = match &mut writer {
            Some(writer) => writer,
            None => writer.insert(Encoder::open(out, encoder, time_base, size, coded, passed.frame)?),
        };
        let timestamp = passed.timestamp - start;
        writer.add(passed.frame, timestamp)?;
        told += 1;
        if told.is_multiple_of(30) {
            progress(timestamp as f64 * seconds);
        }
        Ok(true)
    })?;
    if (steps.cancelled)() {
        return Err(stopped());
    }
    let Some(writer) = writer else {
        return Ok(None);
    };
    let frames = writer.frames;
    writer.finish()?;
    progress(f64::INFINITY);
    Ok(Some(Written {
        frames,
        encoder: encoder.to_owned(),
        size,
    }))
}

/// Joins proxy files written by [`write_stretch`], in order, into `out`:
/// their packets copied as they are (no decoding), each lasting until the
/// next begins. The stretches were encoded alike, so their streams are
/// described alike; one that isn't refuses the join, and the build is
/// done in one pass instead.
fn join(parts: &[std::path::PathBuf], out: &Path) -> diag::Result<()> {
    let inputs = parts
        .iter()
        .map(|p| {
            crate::ffi::Input::open(p)
                .map_err(|e| cannot(format!("A part could not be read again: {}.", e.text)))
        })
        .collect::<diag::Result<Vec<_>>>()?;
    let first = inputs
        .first()
        .ok_or_else(|| cannot("There was nothing to join."))?;
    let c_path = out
        .to_str()
        .and_then(|text| CString::new(text).ok())
        .ok_or_else(|| cannot("The cache folder's name has characters that can't be passed on."))?;
    // SAFETY: every context is opened by FFmpeg and owned here (inputs by
    // `Input`, the output freed below on every path); each part has its
    // one picture stream at index 0, written by `Encoder`; packets are
    // unreferenced after each use.
    unsafe {
        let stream_of = |input: &crate::ffi::Input| *(*input.raw()).streams;
        let model = (*stream_of(first)).codecpar;
        let extradata = |par: *const ff::AVCodecParameters| {
            std::slice::from_raw_parts((*par).extradata, (*par).extradata_size.max(0) as usize).to_vec()
        };
        let described = extradata(model);
        for input in &inputs[1..] {
            let par = (*stream_of(input)).codecpar;
            if extradata(par) != described
                || (*par).width != (*model).width
                || (*par).height != (*model).height
            {
                return Err(cannot("The parts were encoded differently and can't be joined."));
            }
        }
        let mut format: *mut ff::AVFormatContext = ptr::null_mut();
        let r =
            ff::avformat_alloc_output_context2(&mut format, ptr::null(), c"mp4".as_ptr(), c_path.as_ptr());
        if r < 0 || format.is_null() {
            return Err(failed("Starting the proxy file", r));
        }
        let result = (|| {
            let stream = ff::avformat_new_stream(format, ptr::null());
            if stream.is_null() {
                return Err(cannot("There is no memory for a stream."));
            }
            let r = ff::avcodec_parameters_copy((*stream).codecpar, model);
            if r < 0 {
                return Err(failed("Describing the picture stream", r));
            }
            (*(*stream).codecpar).codec_tag = 0;
            let in_base = (*stream_of(first)).time_base;
            (*stream).time_base = in_base;
            let r = ff::avio_open(&mut (*format).pb, c_path.as_ptr(), ff::AVIO_FLAG_WRITE);
            if r < 0 {
                return Err(failed("Opening the proxy file for writing", r));
            }
            let r = ff::avformat_write_header(format, ptr::null_mut());
            if r < 0 {
                return Err(failed("Starting the proxy file", r));
            }
            let out_base = (*stream).time_base;
            let mut packet = ff::av_packet_alloc();
            let mut waiting = ff::av_packet_alloc();
            let mut has_waiting = false;
            let mut last_length = 1;
            let written = (|| {
                for input in &inputs {
                    let from_base = (*stream_of(input)).time_base;
                    while ff::av_read_frame(input.raw(), packet) >= 0 {
                        ff::av_packet_rescale_ts(packet, from_base, out_base);
                        (*packet).stream_index = 0;
                        (*packet).dts = (*packet).pts;
                        if has_waiting {
                            if (*packet).pts <= (*waiting).pts {
                                // Two stretches overlapping: leaving a packet out
                                // could leave the next ones without their key frame.
                                ff::av_packet_unref(packet);
                                return Err(cannot("The parts overlap and can't be joined."));
                            }
                            (*waiting).duration = (*packet).pts - (*waiting).pts;
                            last_length = (*waiting).duration;
                            let r = ff::av_interleaved_write_frame(format, waiting);
                            if r < 0 {
                                return Err(failed("Writing the proxy file", r));
                            }
                        }
                        ff::av_packet_move_ref(waiting, packet);
                        has_waiting = true;
                    }
                }
                if has_waiting {
                    (*waiting).duration = (*waiting).duration.max(last_length).max(1);
                    let r = ff::av_interleaved_write_frame(format, waiting);
                    if r < 0 {
                        return Err(failed("Writing the proxy file", r));
                    }
                }
                let r = ff::av_write_trailer(format);
                if r < 0 {
                    return Err(failed("Completing the proxy file", r));
                }
                Ok(())
            })();
            ff::av_packet_free(&mut packet);
            ff::av_packet_free(&mut waiting);
            written
        })();
        if !(*format).pb.is_null() {
            ff::avio_closep(&mut (*format).pb);
        }
        ff::avformat_free_context(format);
        if result.is_err() {
            let _ = std::fs::remove_file(out);
        }
        result
    }
}

/// The proxy's encoder and file.
struct Encoder {
    format: *mut ff::AVFormatContext,
    codec: *mut ff::AVCodecContext,
    stream: *mut ff::AVStream,
    /// The picture handed to the encoder, NV12 at the proxy's size.
    frame: *mut ff::AVFrame,
    packet: *mut ff::AVPacket,
    /// The last packet out, written once the next one says how long it
    /// lasts: a frame without a length would be left out of the file's
    /// list of what is shown.
    waiting: *mut ff::AVPacket,
    has_waiting: bool,
    /// The length of the last packet written, in the encoder's time base.
    last_length: i64,
    scaler: *mut ff::SwsContext,
    /// Timestamps handed in and not yet come out: with no frame depending
    /// on a later one, they come out in the same order, unchanged.
    sent: VecDeque<i64>,
    last: Option<i64>,
    frames: u64,
    size: (u32, u32),
}

impl Encoder {
    /// Opens `name` for pictures `size` big at `time_base`, tagged with the
    /// colour of `first` (the original's first frame, `coded` big), and
    /// starts the file.
    fn open(
        out: &Path,
        name: &str,
        time_base: ff::AVRational,
        size: (u32, u32),
        coded: (u32, u32),
        first: *const ff::AVFrame,
    ) -> diag::Result<Encoder> {
        let c_path = out
            .to_str()
            .and_then(|text| CString::new(text).ok())
            .ok_or_else(|| cannot("The cache folder's name has characters that can't be passed on."))?;
        let c_name = CString::new(name).map_err(|_| cannot("An encoder's name is not text."))?;
        let mut this = Encoder {
            format: ptr::null_mut(),
            codec: ptr::null_mut(),
            stream: ptr::null_mut(),
            frame: ptr::null_mut(),
            packet: ptr::null_mut(),
            waiting: ptr::null_mut(),
            has_waiting: false,
            last_length: 0,
            scaler: ptr::null_mut(),
            sent: VecDeque::new(),
            last: None,
            frames: 0,
            size,
        };
        // SAFETY: every pointer made here is owned by `this` (freed in its
        // `drop` on any early return); the codec is FFmpeg's static data;
        // the context is filled with plain values before it is opened; the
        // C strings outlive the calls; `first` is a decoded frame in memory.
        unsafe {
            let r = ff::avformat_alloc_output_context2(
                &mut this.format,
                ptr::null(),
                c"mp4".as_ptr(),
                c_path.as_ptr(),
            );
            if r < 0 || this.format.is_null() {
                return Err(failed("Starting the proxy file", r));
            }
            let codec = ff::avcodec_find_encoder_by_name(c_name.as_ptr());
            if codec.is_null() {
                return Err(cannot(format!("This build has no {name} encoder.")));
            }
            this.codec = ff::avcodec_alloc_context3(codec);
            this.packet = ff::av_packet_alloc();
            this.waiting = ff::av_packet_alloc();
            this.frame = ff::av_frame_alloc();
            if this.codec.is_null() || this.packet.is_null() || this.waiting.is_null() || this.frame.is_null()
            {
                return Err(cannot("There is no memory for it."));
            }
            let c = &mut *this.codec;
            c.width = size.0 as i32;
            c.height = size.1 as i32;
            c.pix_fmt = ff::AVPixelFormat::AV_PIX_FMT_NV12;
            c.time_base = time_base;
            c.gop_size = KEY_EVERY;
            c.keyint_min = KEY_EVERY;
            c.max_b_frames = 0;
            // As many threads as the encoder finds useful (FFmpeg's default is one).
            c.thread_count = 0;
            let src = &*first;
            c.color_range = if src.color_range == ff::AVColorRange::AVCOL_RANGE_JPEG {
                ff::AVColorRange::AVCOL_RANGE_JPEG
            } else {
                ff::AVColorRange::AVCOL_RANGE_MPEG
            };
            // The proxy is small: a reader guessing the colour matrix from
            // its size would guess differently from the original's, so the
            // original's (or the guess for it) is written down.
            c.colorspace = match src.colorspace {
                ff::AVColorSpace::AVCOL_SPC_UNSPECIFIED if is_high_definition(coded.0, coded.1) => {
                    ff::AVColorSpace::AVCOL_SPC_BT709
                }
                ff::AVColorSpace::AVCOL_SPC_UNSPECIFIED => ff::AVColorSpace::AVCOL_SPC_BT470BG,
                said => said,
            };
            c.color_primaries = src.color_primaries;
            c.color_trc = src.color_trc;
            if (*(*this.format).oformat).flags & ff::AVFMT_GLOBALHEADER != 0 {
                c.flags |= ff::AV_CODEC_FLAG_GLOBAL_HEADER as i32;
            }
            if name == SOFTWARE_ENCODER {
                // As in the measurements: quick, and small enough.
                ff::av_opt_set(c.priv_data, c"preset".as_ptr(), c"ultrafast".as_ptr(), 0);
                ff::av_opt_set(c.priv_data, c"crf".as_ptr(), c"26".as_ptr(), 0);
                ff::av_opt_set(c.priv_data, c"x264-params".as_ptr(), c"scenecut=0".as_ptr(), 0);
            } else {
                // About what the CPU's encoder spends on a 540p picture at 60 frames a second.
                c.bit_rate = 4_500_000;
                c.rc_max_rate = 9_000_000;
                c.rc_buffer_size = 9_000_000;
            }
            let r = ff::avcodec_open2(this.codec, codec, ptr::null_mut());
            if r < 0 {
                return Err(not_usable(name, r));
            }
            let f = &mut *this.frame;
            f.format = ff::AVPixelFormat::AV_PIX_FMT_NV12 as i32;
            f.width = c.width;
            f.height = c.height;
            let r = ff::av_frame_get_buffer(this.frame, 0);
            if r < 0 {
                return Err(failed("Making room for a picture", r));
            }
            this.stream = ff::avformat_new_stream(this.format, ptr::null());
            if this.stream.is_null() {
                return Err(cannot("There is no memory for a stream."));
            }
            let r = ff::avcodec_parameters_from_context((*this.stream).codecpar, this.codec);
            if r < 0 {
                return Err(failed("Describing the picture stream", r));
            }
            (*this.stream).time_base = time_base;
            let r = ff::avio_open(&mut (*this.format).pb, c_path.as_ptr(), ff::AVIO_FLAG_WRITE);
            if r < 0 {
                return Err(failed("Opening the proxy file for writing", r));
            }
            let r = ff::avformat_write_header(this.format, ptr::null_mut());
            if r < 0 {
                return Err(failed("Starting the proxy file", r));
            }
        }
        Ok(this)
    }

    /// Scales `source` (a decoded frame in memory) to the proxy's size and
    /// encodes it with `timestamp`. A frame whose timestamp is not after
    /// the last one's is left out: a stream can't show two frames at once.
    fn add(&mut self, source: *const ff::AVFrame, timestamp: i64) -> diag::Result<()> {
        if self.last.is_some_and(|last| timestamp <= last) {
            return Ok(());
        }
        // SAFETY: `source` is a decoded frame in ordinary memory; the scaler
        // and the frame are owned by self, the frame allocated as NV12 at
        // the proxy's size and made writable before it is written.
        unsafe {
            let src = &*source;
            if src.width <= 0 || src.height <= 0 {
                return Ok(());
            }
            let format = std::mem::transmute::<i32, ff::AVPixelFormat>(src.format);
            self.scaler = ff::sws_getCachedContext(
                self.scaler,
                src.width,
                src.height,
                format,
                self.size.0 as i32,
                self.size.1 as i32,
                ff::AVPixelFormat::AV_PIX_FMT_NV12,
                ff::SwsFlags::SWS_BILINEAR as i32,
                ptr::null_mut(),
                ptr::null_mut(),
                ptr::null(),
            );
            if self.scaler.is_null() {
                return Err(cannot("The original's picture format can't be scaled."));
            }
            self.keep_range(src.color_range == ff::AVColorRange::AVCOL_RANGE_JPEG);
            let r = ff::av_frame_make_writable(self.frame);
            if r < 0 {
                return Err(failed("Making room for a picture", r));
            }
            ff::sws_scale(
                self.scaler,
                src.data.as_ptr() as *const *const u8,
                src.linesize.as_ptr(),
                0,
                src.height,
                (*self.frame).data.as_ptr(),
                (*self.frame).linesize.as_ptr(),
            );
            (*self.frame).pts = timestamp;
        }
        self.sent.push_back(timestamp);
        self.last = Some(timestamp);
        self.frames += 1;
        self.encode(self.frame)
    }

    /// Scaling YUV to YUV keeps the values as they are: the same range out
    /// as in, so a full-range original stays full range.
    ///
    /// # Safety
    /// The scaler must have been made.
    unsafe fn keep_range(&mut self, full: bool) {
        // SAFETY: the scaler is valid; the tables it returns are its own.
        unsafe {
            let (mut inverse, mut table) = (ptr::null_mut(), ptr::null_mut());
            let (mut src_range, mut dst_range, mut brightness, mut contrast, mut saturation) =
                (0, 0, 0, 0, 0);
            if ff::sws_getColorspaceDetails(
                self.scaler,
                &mut inverse,
                &mut src_range,
                &mut table,
                &mut dst_range,
                &mut brightness,
                &mut contrast,
                &mut saturation,
            ) < 0
            {
                return;
            }
            let range = i32::from(full);
            ff::sws_setColorspaceDetails(
                self.scaler,
                inverse,
                range,
                table,
                range,
                brightness,
                contrast,
                saturation,
            );
        }
    }

    /// Hands `frame` (null: no more) to the encoder and writes what comes
    /// out, checking every packet keeps the time its frame went in with.
    fn encode(&mut self, frame: *const ff::AVFrame) -> diag::Result<()> {
        // SAFETY: the context is an opened encoder, the frame null or a
        // filled frame of its format, the packet ours and unreferenced after
        // each use, the stream part of the open file.
        unsafe {
            let r = ff::avcodec_send_frame(self.codec, frame);
            if r < 0 && r != ff::AVERROR_EOF {
                return Err(failed("Encoding", r));
            }
            loop {
                let r = ff::avcodec_receive_packet(self.codec, self.packet);
                if r == ff::AVERROR(ff::EAGAIN) || r == ff::AVERROR_EOF {
                    return Ok(());
                }
                if r < 0 {
                    return Err(failed("Encoding", r));
                }
                let pts = (*self.packet).pts;
                if self.sent.pop_front() != Some(pts) {
                    ff::av_packet_unref(self.packet);
                    return Err(cannot("The encoder changed the frames' times."));
                }
                (*self.packet).dts = pts;
                // The one before lasts until this one.
                if self.has_waiting {
                    (*self.waiting).duration = pts - (*self.waiting).pts;
                    self.last_length = (*self.waiting).duration;
                    self.write_waiting()?;
                }
                ff::av_packet_move_ref(self.waiting, self.packet);
                self.has_waiting = true;
            }
        }
    }

    /// Writes the packet held back, its length set.
    fn write_waiting(&mut self) -> diag::Result<()> {
        if !self.has_waiting {
            return Ok(());
        }
        self.has_waiting = false;
        // SAFETY: `waiting` holds a packet from this encoder, in its time
        // base; the muxer takes its contents, and it is unreferenced after.
        unsafe {
            ff::av_packet_rescale_ts(self.waiting, (*self.codec).time_base, (*self.stream).time_base);
            (*self.waiting).stream_index = (*self.stream).index;
            let r = ff::av_interleaved_write_frame(self.format, self.waiting);
            ff::av_packet_unref(self.waiting);
            if r < 0 {
                return Err(failed("Writing the proxy file", r));
            }
        }
        Ok(())
    }

    /// Encodes what is still waiting and completes the file.
    fn finish(mut self) -> diag::Result<()> {
        self.encode(ptr::null())?;
        // The last frame lasts as long as the one before it.
        // SAFETY: `waiting` is ours; only its plain fields are read and set.
        unsafe {
            if self.has_waiting && (*self.waiting).duration <= 0 {
                (*self.waiting).duration = self.last_length.max(1);
            }
        }
        self.write_waiting()?;
        // SAFETY: the header was written, so the trailer may be; the file is
        // closed here and not again in `drop` (`avio_closep` nulls `pb`).
        unsafe {
            let r = ff::av_write_trailer(self.format);
            if r < 0 {
                return Err(failed("Completing the proxy file", r));
            }
            let r = ff::avio_closep(&mut (*self.format).pb);
            if r < 0 {
                return Err(failed("Closing the proxy file", r));
            }
        }
        Ok(())
    }
}

impl Drop for Encoder {
    fn drop(&mut self) {
        // SAFETY: each pointer is null or was allocated by the matching
        // FFmpeg call and is freed once (the calls null it, or it is nulled
        // here); the stream belongs to the format context.
        unsafe {
            ff::sws_freeContext(self.scaler);
            self.scaler = ptr::null_mut();
            ff::av_frame_free(&mut self.frame);
            ff::av_packet_free(&mut self.packet);
            ff::av_packet_free(&mut self.waiting);
            ff::avcodec_free_context(&mut self.codec);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_encoder_that_does_not_open_is_not_blamed_on_the_disk() {
        let err = not_usable("h264_nvenc", -22);
        assert!(err.why_text().is_some_and(|w| w.contains("h264_nvenc")));
        assert!(!err.fix_text().contains("disk"), "{}", err.fix_text());
        assert!(err.fix_text().contains("next encoder"));
    }

    fn fixture(name: &str) -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(name)
    }

    /// Every frame's time in seconds, from the stream's zero as the
    /// preview counts (`from_zero`: a proxy's own times, not shifted).
    fn frame_times(decoder: &mut VideoDecoder, from_zero: bool) -> Vec<f64> {
        let tb = decoder.time_base_rational();
        let start = if from_zero { 0 } else { decoder.start_timestamp() };
        let mut times = Vec::new();
        decoder
            .each_frame(&mut |f| {
                times.push((f.timestamp - start) as f64 * f64::from(tb.num) / f64::from(tb.den));
                Ok(true)
            })
            .unwrap();
        times
    }

    /// Builds without stopping, noting the progress told.
    fn write_all(original: &Path, stream: u32, length: f64, out: &Path, told: &mut Vec<f32>) -> Written {
        let told = std::sync::Mutex::new(told);
        let progress = |done| told.lock().unwrap().push(done);
        let steps = Steps {
            cancelled: &|| false,
            progress: &progress,
            pause: &|| {},
        };
        write(original, stream, length, out, Decoding::Software, true, 1, &steps).unwrap()
    }

    /// The frame number `counted-*` fixtures show in their bars.
    fn counted(frame: &crate::Frame) -> u32 {
        let width = frame.width as usize;
        (0..8).fold(0, |n, bar| {
            let red = frame.rgba[(width * 16 + bar * 16 + 8) * 4];
            if red > 127 { n | 1 << bar } else { n }
        })
    }

    /// Every frame of a proxy as (time, the number its bars show).
    fn counted_frames(proxy: &Path) -> Vec<(f64, u32)> {
        let mut decoder = VideoDecoder::open_proxy(proxy, 0).unwrap();
        let times = frame_times(&mut decoder, true);
        times
            .iter()
            .map(|t| (*t, counted(&decoder.frame_at(*t + 1e-4, 128, 32).unwrap())))
            .collect()
    }

    #[test]
    fn stretches_start_at_key_frames_near_equal_shares() {
        let keys: Vec<f64> = (0..60).map(|k| f64::from(k) * 4.0).collect();
        assert_eq!(stretches(&keys, 240.0, 3, 20.0), [80.0, 160.0]);
        assert_eq!(
            stretches(&keys, 30.0, 3, 20.0),
            Vec::<f64>::new(),
            "too short to split"
        );
        assert_eq!(
            stretches(&[0.0], 240.0, 3, 20.0),
            Vec::<f64>::new(),
            "one key frame: one pass"
        );
        assert_eq!(stretches(&keys, 240.0, 1, 20.0), Vec::<f64>::new());
    }

    #[test]
    fn a_proxy_built_in_stretches_has_every_frame_once_in_order() {
        // Key frames every quarter second, B-frames, and groups of
        // pictures open and closed: stretches meet at key frames whose
        // neighbours are made from both sides.
        for name in ["counted-open-gop.mkv", "counted-b-frames.mkv", "counted-120.mkv"] {
            let original = fixture(name);
            let dir = tempfile::tempdir().unwrap();
            let (one, split) = (dir.path().join("one.mp4"), dir.path().join("split.mp4"));
            let steps = Steps {
                cancelled: &|| false,
                progress: &|_| {},
                pause: &|| {},
            };
            write_with(
                &original,
                0,
                2.0,
                &one,
                Decoding::Software,
                SOFTWARE_ENCODER,
                &steps,
            )
            .unwrap();
            let parts = Split {
                workers: 3,
                shortest: 0.4,
            };
            let written = write_split(
                &original,
                0,
                2.0,
                &split,
                Decoding::Software,
                SOFTWARE_ENCODER,
                parts,
                &steps,
            )
            .unwrap();
            let keys = VideoDecoder::open_for_pass(&original, 0, Decoding::Software, 1)
                .unwrap()
                .listed_key_times();
            let starts = stretches(&keys, 2.0, 3, 0.4).len();
            if name != "counted-b-frames.mkv" {
                assert_eq!(starts, 2, "{name}: built in three stretches");
            }
            let (a, b) = (counted_frames(&one), counted_frames(&split));
            assert_eq!(written.frames as usize, a.len(), "{name}");
            assert_eq!(a.len(), 240, "{name}: two seconds at 120 a second");
            assert_eq!(a, b, "{name}: the same frames at the same times");
            assert!(
                b.iter().enumerate().all(|(i, (_, n))| *n == i as u32),
                "{name}: in order"
            );
        }
    }

    #[test]
    fn the_proxy_is_540_lines_or_less_and_even() {
        assert_eq!(size_for((2560, 1440)), (960, 540));
        assert_eq!(size_for((1920, 1080)), (960, 540));
        assert_eq!(
            size_for((1080, 1920)),
            (540, 960),
            "stored upright: the shorter side counts"
        );
        assert_eq!(size_for((640, 360)), (640, 360), "never enlarged");
        assert_eq!(size_for((3440, 1440)), (1290, 540));
    }

    #[test]
    fn a_proxy_has_exactly_the_originals_frame_times() {
        for name in ["vfr-60.mkv", "game-3-audio.mkv", "phone-rotated.mp4"] {
            let original = fixture(name);
            let dir = tempfile::tempdir().unwrap();
            let out = dir.path().join("proxy.building");
            let mut told = Vec::new();
            let info = crate::probe(&original).unwrap();
            let video = info.info.video.as_ref().unwrap();
            let written = write_all(
                &original,
                video.index,
                info.duration.as_secs_f64(),
                &out,
                &mut told,
            );
            assert_eq!(written.encoder, SOFTWARE_ENCODER);
            assert_eq!(told.last(), Some(&1.0), "{name}");
            assert!(
                told.windows(2).all(|w| w[0] <= w[1]),
                "{name}: progress only grows"
            );
            let mut original_decoder =
                VideoDecoder::open(&original, video.index, 0, Decoding::Software).unwrap();
            let mut proxy = VideoDecoder::open_proxy(&out, video.rotation).unwrap();
            let want = frame_times(&mut original_decoder, false);
            let got = frame_times(&mut proxy, true);
            assert_eq!(got.len(), want.len(), "{name}: one frame for every frame");
            assert_eq!(written.frames as usize, want.len());
            for (g, w) in got.iter().zip(&want) {
                assert!((g - w).abs() < 1e-6, "{name}: {g} for {w}");
            }
            // Key frames every 8 frames, and nothing else.
            assert!(proxy.key_frames().len() >= want.len() / 8, "{name}");
        }
    }

    #[test]
    fn a_proxy_frame_is_the_originals_frame_at_the_same_moment() {
        let original = fixture("vfr-60.mkv");
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("proxy.mp4");
        let info = crate::probe(&original).unwrap();
        write_all(&original, 0, info.duration.as_secs_f64(), &out, &mut Vec::new());
        let mut a = VideoDecoder::open(&original, 0, 0, Decoding::Software).unwrap();
        let mut b = VideoDecoder::open_proxy(&out, 0).unwrap();
        for t in [0.0, 0.51, 1.234, 0.2, 1.9, 0.75] {
            let x = a.picture_at(t, 640, 360).unwrap();
            let y = b.picture_at(t, 640, 360).unwrap();
            assert!(
                (x.time() - y.time()).abs() < 1e-6,
                "at {t}: {} and {}",
                x.time(),
                y.time()
            );
        }
    }

    #[test]
    fn a_stopped_build_leaves_nothing() {
        let original = fixture("game-3-audio.mkv");
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("proxy.mp4");
        let frames = std::sync::atomic::AtomicU32::new(0);
        let stop = || frames.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1 > 5;
        let result = write(
            &original,
            0,
            10.0,
            &out,
            Decoding::Software,
            true,
            1,
            &Steps {
                cancelled: &stop,
                progress: &|_| {},
                pause: &|| {},
            },
        );
        assert_eq!(result.unwrap_err().what(), stopped().what());
        assert!(!out.exists());
    }
}
