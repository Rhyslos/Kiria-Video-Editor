//! Turning a sound stream into samples: stereo, 32-bit float, at the rate
//! the sound card plays, from any point of the file.
//!
//! [`AudioDecoder::seek`] goes to a time; [`AudioDecoder::read`] then fills
//! a buffer with the samples from there on.
//!
//! **Time 0 is the picture's start.** In a file with a picture, sound is
//! timed from the picture stream's first timestamp, so sound that starts a
//! little before the picture (a few milliseconds is common in recordings)
//! is heard that much before it, and sound that starts later is preceded by
//! silence, as any player plays the file. A file of sound alone is timed
//! from its own start.
//!
//! **Samples are counted, not taken from each packet's time.** Containers
//! round timestamps (to a millisecond in MKV, 48 samples at 48 kHz), so the
//! timestamps of a steady stream wobble around its sample count. The first
//! frame after opening or a jump is placed by its timestamp; every frame
//! after it follows on, sample for sample. Only when a frame's timestamp is
//! more than [`RESYNC`] away from where counting puts it (a hole in the
//! recording, or sound whose clock drifts) is it placed by its timestamp
//! again: silence fills a hole, overlapping sound is dropped.
//!
//! **A jump lands on the exact sample a straight read reaches.** While
//! opening, the first two frames are decoded (a read from the start gives
//! them): where the second starts and how long it is give the stream's grid
//! of frames. After a jump, the first frame's rounded timestamp is moved to
//! the grid point it rounds from, so it is counted exactly as reading from
//! the start would count it. (Where frames vary in length the timestamp is
//! used as it is.) Sound read at another rate than the file's goes through a
//! resampler that starts afresh after a jump, so there the first few samples
//! may differ slightly.
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
use std::path::{Path, PathBuf};
use std::ptr;

/// Samples come out as interleaved stereo.
pub const CHANNELS: usize = 2;

/// Asked for as the rate: the stream's own, so nothing is resampled
/// ([`AudioDecoder::rate`] says which it is).
pub const OWN_RATE: u32 = 0;

/// How far before the target a seek asks the file to go (seconds). Sound
/// and picture for the same moment sit close together in a file but not in
/// a fixed order; this covers the difference, and gives a decoder that
/// needs a little sound before the target to settle (Opus) that sound.
const SEEK_MARGIN: f64 = 0.5;

/// How far a frame's timestamp may be from where counting the samples puts
/// it before the timestamp is believed instead (seconds). More than any
/// container's rounding, less than a picture frame at 60 per second.
const RESYNC: f64 = 0.010;

/// When each stream of a file starts and when the file does (seconds of
/// file time): what probing a file finds that its header may not say, and
/// all a decoder takes from probing besides the sound's own settings.
#[derive(Debug, Clone, PartialEq)]
struct Starts {
    streams: Vec<Option<f64>>,
    file: Option<f64>,
}

/// The size and change time of a file, so facts kept about it are dropped
/// when it changes.
type Stamp = (u64, Option<std::time::SystemTime>);

/// Each file's [`Starts`], probed once a session: opening a sound stream
/// again (another clip of the file, a jump to it, each time playback
/// starts) then only reads the file's header. Probing a large recording
/// decodes some of its pictures and takes tens of milliseconds; the header
/// alone a fraction of one.
static PROBED: std::sync::Mutex<Option<std::collections::HashMap<PathBuf, (Stamp, Starts)>>> =
    std::sync::Mutex::new(None);

/// Opens from the header alone, counted for the tests.
#[cfg(test)]
static FROM_HEADER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

fn stamp(path: &Path) -> Option<Stamp> {
    let meta = std::fs::metadata(path).ok()?;
    Some((meta.len(), meta.modified().ok()))
}

/// The file opened for reading `stream`, and its streams' start times:
/// from its header alone when they were probed before this session and the
/// header holds all the stream needs, else probed (and kept).
fn open_input(path: &Path, stream: u32) -> Result<(Input, Starts), crate::ffi::OpenError> {
    let stamp = stamp(path);
    let known = stamp.and_then(|stamp| {
        let probed = PROBED.lock().unwrap_or_else(|p| p.into_inner());
        probed
            .as_ref()?
            .get(path)
            .filter(|(at, _)| *at == stamp)
            .map(|(_, starts)| starts.clone())
    });
    if let Some(starts) = known
        && let Ok(input) = Input::open_unprobed(path)
        // SAFETY: `input` was just opened and owns its context.
        && unsafe { header_suffices(input.raw(), stream, starts.streams.len()) }
    {
        #[cfg(test)]
        FROM_HEADER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        return Ok((input, starts));
    }
    let input = Input::open(path)?;
    // SAFETY: just opened and probed; owned by `input`.
    let starts = unsafe { starts_of(input.raw()) };
    if let Some(stamp) = stamp {
        let mut probed = PROBED.lock().unwrap_or_else(|p| p.into_inner());
        probed
            .get_or_insert_with(Default::default)
            .insert(path.to_path_buf(), (stamp, starts.clone()));
    }
    Ok((input, starts))
}

/// Whether the header of `raw`, opened without probing, holds everything
/// sound stream `stream` needs: the same number of streams as probing
/// found, and the sound's codec, rate and channels.
///
/// # Safety
/// `raw` is an opened format context.
unsafe fn header_suffices(raw: *mut ff::AVFormatContext, stream: u32, streams: usize) -> bool {
    // SAFETY: the caller's contract; `streams` has `nb_streams` entries,
    // each with codec parameters.
    unsafe {
        if (*raw).nb_streams as usize != streams || stream >= (*raw).nb_streams {
            return false;
        }
        let par = (**(*raw).streams.add(stream as usize)).codecpar;
        (*par).codec_id != ff::AVCodecID::AV_CODEC_ID_NONE
            && (*par).sample_rate > 0
            && (*par).ch_layout.nb_channels > 0
    }
}

/// Every stream's start time and the file's, from a probed context.
///
/// # Safety
/// `raw` is an opened, probed format context.
unsafe fn starts_of(raw: *mut ff::AVFormatContext) -> Starts {
    // SAFETY: the caller's contract; `streams` has `nb_streams` entries.
    unsafe {
        let streams = (0..(*raw).nb_streams)
            .map(|i| {
                let st = *(*raw).streams.add(i as usize);
                let base = f64::from((*st).time_base.num) / f64::from((*st).time_base.den.max(1));
                ((*st).start_time != ff::AV_NOPTS_VALUE).then(|| (*st).start_time as f64 * base)
            })
            .collect();
        let file = ((*raw).start_time != ff::AV_NOPTS_VALUE)
            .then(|| (*raw).start_time as f64 / f64::from(ff::AV_TIME_BASE));
        Starts { streams, file }
    }
}

/// Makes the demuxer skip every stream of `raw` but those in `wanted`: the
/// file's other streams (the picture above all, many times the sound's
/// size) are passed over instead of being read into packets that are then
/// dropped. Searching by the picture's index still works.
///
/// # Safety
/// `raw` is an opened format context.
unsafe fn read_only(raw: *mut ff::AVFormatContext, wanted: &[u32]) {
    // SAFETY: the caller's contract; `streams` has `nb_streams` entries.
    unsafe {
        for i in 0..(*raw).nb_streams {
            let discard = if wanted.contains(&i) {
                ff::AVDiscard::AVDISCARD_DEFAULT
            } else {
                ff::AVDiscard::AVDISCARD_ALL
            };
            (**(*raw).streams.add(i as usize)).discard = discard;
        }
    }
}

pub struct AudioDecoder {
    input: Input,
    path: PathBuf,
    codec: *mut ff::AVCodecContext,
    stream: i32,
    /// The stream a seek searches by, and its time base: the picture when
    /// the file has one, else the sound itself.
    seek_stream: i32,
    seek_time_base: f64,
    time_base: f64,
    /// File time (seconds) of time 0: the picture's first timestamp when the
    /// file has a picture, else the sound's own.
    origin: f64,
    /// File time (seconds) of the first thing in the file.
    file_start: f64,
    rate: u32,
    /// The stream's time base in samples, rounded up: how far rounding can
    /// move a timestamp.
    tick: i64,
    resampler: *mut ff::SwrContext,
    frame: *mut ff::AVFrame,
    packet: *mut ff::AVPacket,
    /// Decoded, converted samples waiting to be read (interleaved stereo).
    pending: VecDeque<f32>,
    /// Sample frames of silence to give before `pending`: the sound in the
    /// file starts this much after the time that was asked for.
    silence: usize,
    /// The sample (at `rate`, counted from time 0) `read` gives next.
    next: i64,
    /// The sample just after the last decoded frame: where the next one
    /// follows on. `None` until the first frame after opening or a jump.
    follows: Option<i64>,
    grid: Grid,
    seeks: u64,
    /// Nothing read or jumped to since opening: it is at time 0 already.
    untouched: bool,
    ended: bool,
}

/// Where the stream's frames start, learnt from its first two frames.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Grid {
    /// While opening: where the first frame ended, once it is decoded.
    Learning(Option<i64>),
    /// Frames start at `first` plus a whole number of `size` samples.
    Known { first: i64, size: i64 },
    /// Unknown (a stream of one frame, or read at another rate than the
    /// file's): timestamps are taken as they are.
    Unknown,
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
    /// Opens sound stream `stream` of the file, to be read at `rate`
    /// samples per second from time 0 (the picture's start).
    pub fn open(path: &Path, stream: u32, rate: u32) -> diag::Result<AudioDecoder> {
        let mut this = AudioDecoder::open_unread(path, stream, rate)?;
        this.learn_grid();
        Ok(this)
    }

    /// Opened, with nothing decoded yet: to be handed packets by someone
    /// else ([`AudioStreams`]), who reads the file from its start.
    fn open_unread(path: &Path, stream: u32, rate: u32) -> diag::Result<AudioDecoder> {
        let (input, starts) = open_input(path, stream).map_err(|e| {
            AppError::new("The sound can't be played")
                .why(format!("{}: {}.", path.display(), e.text))
                .fix("Check that the file still exists and that the drive is connected.")
        })?;
        let raw = input.raw();
        // SAFETY: `raw` is an opened context owned by `input`; pointers from
        // FFmpeg are checked before use; contexts are freed in Drop.
        let this = unsafe {
            if stream >= (*raw).nb_streams {
                return Err(AppError::new("The sound can't be played")
                    .why(format!("{} has no stream {stream}.", path.display()))
                    .fix("Import the file again."));
            }
            let st = *(*raw).streams.add(stream as usize);
            let par = (*st).codecpar;
            let rate = match rate {
                OWN_RATE => u32::try_from((*par).sample_rate).unwrap_or(0),
                rate => rate,
            }
            .max(8_000);
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
            let start = |i: u32| starts.streams.get(i as usize).copied().flatten();
            let picture = picture_stream(raw);
            let seek_stream = picture.unwrap_or(stream);
            let origin = picture.and_then(start).or_else(|| start(stream)).unwrap_or(0.0);
            let file_start = starts.file.unwrap_or(origin);
            let this = AudioDecoder {
                input,
                path: path.to_path_buf(),
                file_start,
                codec,
                stream: stream as i32,
                seek_stream: seek_stream as i32,
                seek_time_base: time_base(*(*raw).streams.add(seek_stream as usize)),
                time_base: time_base(st),
                origin,
                rate,
                tick: ((time_base(st) * f64::from(rate)).ceil() as i64).max(1),
                resampler: ptr::null_mut(),
                frame: ff::av_frame_alloc(),
                packet: ff::av_packet_alloc(),
                pending: VecDeque::new(),
                silence: 0,
                next: 0,
                follows: None,
                grid: Grid::Learning(None),
                seeks: 0,
                untouched: true,
                ended: false,
            };
            if codec.is_null() {
                return Err(failed(path, "Setting up the decoder", ff::AVERROR(ff::ENOMEM)));
            }
            let r = ff::avcodec_parameters_to_context(codec, par);
            if r < 0 {
                return Err(failed(path, "Reading the sound settings", r));
            }
            // With the packets' time base the decoder moves a frame's
            // timestamp past the samples it leaves out at the start (an
            // Opus or AAC encoder's priming), so the first sound heard is
            // timed where it belongs.
            (*codec).pkt_timebase = (*st).time_base;
            read_only(raw, &[stream]);
            let r = ff::avcodec_open2(codec, decoder, ptr::null_mut());
            if r < 0 {
                return Err(failed(path, "Opening the decoder", r));
            }
            this
        };
        Ok(this)
    }

    /// Decodes the first two frames (kept for reading) to learn where the
    /// stream's frames lie.
    fn learn_grid(&mut self) {
        while matches!(self.grid, Grid::Learning(_)) && !self.ended {
            if let Err(err) = self.decode_more() {
                tracing::warn!("{}", err.report());
                self.ended = true;
            }
        }
        if matches!(self.grid, Grid::Learning(_)) {
            self.grid = Grid::Unknown;
        }
    }

    /// The rate samples come out at.
    pub fn rate(&self) -> u32 {
        self.rate
    }

    /// Time (seconds from time 0) of the next sample `read` returns.
    pub fn position(&self) -> f64 {
        self.next as f64 / f64::from(self.rate)
    }

    /// The next sample `read` returns, counted from time 0 at the rate the
    /// decoder reads at: exact, for telling whether reading carries on.
    pub fn next_sample(&self) -> i64 {
        self.next
    }

    /// How many times the decoder has jumped since it was opened.
    pub fn seeks(&self) -> u64 {
        self.seeks
    }

    /// Goes to `seconds` from time 0, to the nearest sample. The next sample
    /// read is the one at that time; where the file has no sound there (it
    /// starts later, or has a hole), silence comes first.
    pub fn seek(&mut self, seconds: f64) {
        self.seek_to_sample((seconds.max(0.0) * f64::from(self.rate)).round() as i64);
    }

    /// Goes to sample `target` (at the decoder's rate, from time 0).
    pub fn seek_to_sample(&mut self, target: i64) {
        let target = target.max(0);
        if target == 0 && self.untouched {
            return; // already there, with the first frames decoded
        }
        self.untouched = false;
        let from = target as f64 / f64::from(self.rate) + self.origin - SEEK_MARGIN;
        if from < self.origin.min(self.file_start) && self.rewind() {
            // From the very start: opened again (see `rewind`).
        } else {
            self.search(target, from);
        }
        // SAFETY: valid contexts owned by self; the resampler is freed and
        // made again for the next frame so no old samples leak through.
        unsafe {
            ff::avcodec_flush_buffers(self.codec);
            ff::swr_free(&mut self.resampler);
        }
        self.pending.clear();
        self.silence = 0;
        self.next = target;
        self.follows = None;
        self.seeks += 1;
        self.ended = false;
    }

    /// Goes back to the file's very start by opening it again. A search
    /// can't: it lands on the first picture's key frame and some containers
    /// (MKV) then drop every packet timed before it, such as sound that
    /// starts a few milliseconds before the picture, which is heard, and the
    /// Opus packet before the first that a decoder needs to settle. Whether
    /// it worked.
    fn rewind(&mut self) -> bool {
        match open_input(&self.path, self.stream as u32) {
            Ok((input, _)) => {
                // SAFETY: the context was just opened and is owned by `input`.
                unsafe { read_only(input.raw(), &[self.stream as u32]) };
                self.input = input;
                true
            }
            Err(e) => {
                tracing::warn!(
                    "the sound could not go back to its start ({}); searching instead",
                    e.text
                );
                false
            }
        }
    }

    /// Asks the file to go to `from` seconds of file time or before, to
    /// read on from there to sample `target`.
    fn search(&mut self, target: i64, from: f64) {
        // SAFETY: the context is valid and owned by self.
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
                    "the sound could not jump to sample {target} ({}); it carries on from where it was",
                    error_text(found)
                );
            }
        }
    }

    /// Fills `out` (interleaved stereo) with the next samples. Returns how
    /// many sample frames were written; fewer than asked means the end.
    pub fn read(&mut self, out: &mut [f32]) -> usize {
        let wanted = out.len() / CHANNELS;
        self.untouched = false;
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
        self.next += (quiet + frames) as i64;
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

    /// Decodes `packet` (a packet of this decoder's stream; null: the file
    /// has ended) and converts what comes out into `pending`.
    ///
    /// # Safety
    /// `packet` is null or a packet of this decoder's stream.
    unsafe fn feed(&mut self, packet: *const ff::AVPacket) -> diag::Result<()> {
        // SAFETY: send/receive contract followed; each frame is
        // unreferenced after converting.
        unsafe {
            loop {
                let sent = ff::avcodec_send_packet(self.codec, packet);
                loop {
                    let r = ff::avcodec_receive_frame(self.codec, self.frame);
                    if r != 0 {
                        if r == ff::AVERROR_EOF {
                            self.ended = true;
                        }
                        break;
                    }
                    let result = self.convert();
                    ff::av_frame_unref(self.frame);
                    result?;
                }
                // Full: what was decoded is out now, so it takes the packet.
                if sent != ff::AVERROR(ff::EAGAIN) {
                    return Ok(());
                }
            }
        }
    }

    /// Moves the samples decoded so far (silence first, where the sound
    /// starts later) onto the end of `out`.
    fn take(&mut self, out: &mut Vec<f32>) {
        out.extend(std::iter::repeat_n(0.0, self.silence * CHANNELS));
        let frames = self.silence + self.pending.len() / CHANNELS;
        out.extend(self.pending.drain(..));
        self.silence = 0;
        self.next += frames as i64;
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
            let stamp = (ts != ff::AV_NOPTS_VALUE)
                .then(|| ((ts as f64 * self.time_base - self.origin) * f64::from(self.rate)).round() as i64);
            self.queue(&buf, stamp, f.sample_rate == self.rate as i32);
            Ok(())
        }
    }

    /// Places a converted frame (interleaved stereo) whose timestamp, in
    /// samples from time 0, is `stamp`, and queues what of it is wanted.
    /// `same_rate`: read at the file's own rate, so its grid applies.
    fn queue(&mut self, samples: &[f32], stamp: Option<i64>, same_rate: bool) {
        let n = (samples.len() / CHANNELS) as i64;
        let resync = ((RESYNC * f64::from(self.rate)) as i64).max(self.tick + 1);
        let at = match (self.follows, stamp) {
            // Carrying on: counted, unless the timestamp is far off.
            (Some(follows), Some(t)) if (t - follows).abs() > resync => self.on_grid(t),
            (Some(follows), _) => follows,
            // The first frame after opening or a jump.
            (None, Some(t)) => self.on_grid(t),
            (None, None) => self.next,
        };
        self.grid = match self.grid {
            Grid::Learning(None) => Grid::Learning(Some(at + n)),
            Grid::Learning(Some(first)) if same_rate && at == first => Grid::Known { first, size: n },
            Grid::Learning(Some(_)) => Grid::Unknown,
            grid => grid,
        };
        self.follows = Some(at + n);
        let queued = self.next + (self.silence + self.pending.len() / CHANNELS) as i64;
        match place(queued, at, n) {
            Placed::Before => {}
            Placed::Across { drop } => self.pending.extend(&samples[drop * CHANNELS..]),
            Placed::After { silence } => {
                if self.pending.is_empty() {
                    self.silence += silence;
                } else {
                    // A hole in the sound: silence between what is queued and this.
                    self.pending.extend(std::iter::repeat_n(0.0, silence * CHANNELS));
                }
                self.pending.extend(samples);
            }
        }
    }

    /// A frame's rounded timestamp `stamp` (samples) moved to the grid
    /// point it rounds from, when there is one within rounding.
    fn on_grid(&self, stamp: i64) -> i64 {
        snap(self.grid, stamp, self.tick)
    }
}

/// `stamp` moved to the nearest point of `grid` when that is within the
/// rounding of a timestamp (`tick` samples); else `stamp` as it is.
fn snap(grid: Grid, stamp: i64, tick: i64) -> i64 {
    let Grid::Known { first, size } = grid else {
        return stamp;
    };
    if size <= 0 {
        return stamp;
    }
    let point = first + ((stamp - first) as f64 / size as f64).round() as i64 * size;
    if (point - stamp).abs() <= tick + 1 {
        point
    } else {
        stamp
    }
}

/// Where a decoded frame lies against where the queue ends.
#[derive(Debug, PartialEq, Eq)]
enum Placed {
    /// All of it is before: dropped.
    Before,
    /// The queue's end is inside it: this many sample frames are dropped.
    Across { drop: usize },
    /// It starts after the queue's end: this much silence comes first.
    After { silence: usize },
}

/// A frame of `frames` samples starting at sample `at`, against a queue
/// that ends at sample `queued`: sound before the queue's end is dropped,
/// and sound that starts after it is preceded by silence.
fn place(queued: i64, at: i64, frames: i64) -> Placed {
    let early = queued - at;
    if early >= frames {
        Placed::Before
    } else if early >= 0 {
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

/// Several sound streams of one file read start to end together, the file
/// read once: for work that needs all of a long recording's streams (its
/// waveforms), which would otherwise read the whole file, picture and all,
/// once per stream. Each stream comes out as [`AudioDecoder::read`] gives
/// it from time 0: stereo, timed from the picture's start.
pub struct AudioStreams {
    input: Input,
    packet: *mut ff::AVPacket,
    /// One per stream asked for, in that order; each is handed its packets.
    decoders: Vec<AudioDecoder>,
    ended: bool,
}

// One thread at a time uses it; FFmpeg contexts may move between threads.
unsafe impl Send for AudioStreams {}

impl std::fmt::Debug for AudioStreams {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "AudioStreams({} streams)", self.decoders.len())
    }
}

impl AudioStreams {
    /// Opens `streams` of the file, each read at `rate` (or [`OWN_RATE`]).
    pub fn open(path: &Path, streams: &[u32], rate: u32) -> diag::Result<AudioStreams> {
        let mut decoders = Vec::with_capacity(streams.len());
        for stream in streams {
            let mut decoder = AudioDecoder::open_unread(path, *stream, rate)?;
            decoder.untouched = false;
            decoders.push(decoder);
        }
        let (input, _) = open_input(path, streams.first().copied().unwrap_or(0)).map_err(|e| {
            AppError::new("The sound can't be read")
                .why(format!("{}: {}.", path.display(), e.text))
                .fix("Check that the file still exists and that the drive is connected.")
        })?;
        // SAFETY: just opened; owned by `input`. The packet is freed in Drop.
        let packet = unsafe {
            read_only(input.raw(), streams);
            ff::av_packet_alloc()
        };
        if packet.is_null() {
            return Err(failed(path, "Setting up the reading", ff::AVERROR(ff::ENOMEM)));
        }
        Ok(AudioStreams {
            input,
            packet,
            decoders,
            ended: streams.is_empty(),
        })
    }

    /// The rate each stream comes out at, in the order asked for.
    pub fn rates(&self) -> Vec<u32> {
        self.decoders.iter().map(AudioDecoder::rate).collect()
    }

    /// Reads on until about `frames` sample frames of some stream are
    /// decoded, then moves every stream's new samples (interleaved stereo)
    /// onto the end of its buffer in `out` (one per stream). Returns false
    /// once the whole file has been read and handed over.
    pub fn read(&mut self, frames: usize, out: &mut [Vec<f32>]) -> bool {
        if self.ended {
            return false;
        }
        let wanted = frames * CHANNELS;
        while !self.ended && self.decoders.iter().all(|d| d.pending.len() < wanted) {
            // About as many packets as `frames` take (most sound codecs put
            // a thousand or so samples in each).
            let batch = self.read_batch(frames.div_ceil(1024).clamp(1, BATCH));
            decode_batch(&mut self.decoders, batch, self.ended);
        }
        for (decoder, out) in self.decoders.iter_mut().zip(out.iter_mut()) {
            decoder.take(out);
        }
        true
    }
}

impl AudioStreams {
    /// The next packets of every stream asked for, up to `most` for one of
    /// them, each in its decoder's list. Sets `ended` at the file's end.
    fn read_batch(&mut self, most: usize) -> Vec<Vec<Packet>> {
        let mut batch: Vec<Vec<Packet>> = self.decoders.iter().map(|_| Vec::new()).collect();
        while batch.iter().all(|b| b.len() < most) {
            // SAFETY: the context is owned by self; a packet read is moved
            // into one of its own (freed by `Packet`'s Drop), leaving
            // `self.packet` blank for the next.
            unsafe {
                if ff::av_read_frame(self.input.raw(), self.packet) < 0 {
                    self.ended = true;
                    break;
                }
                let stream = (*self.packet).stream_index;
                match self.decoders.iter().position(|d| d.stream == stream) {
                    Some(i) => {
                        let own = ff::av_packet_alloc();
                        if own.is_null() {
                            ff::av_packet_unref(self.packet);
                            continue;
                        }
                        ff::av_packet_move_ref(own, self.packet);
                        batch[i].push(Packet(own));
                    }
                    None => ff::av_packet_unref(self.packet),
                }
            }
        }
        batch
    }
}

/// The most packets of a stream read before they are decoded: about a
/// second of sound, so a thread per stream has work enough to be worth it.
const BATCH: usize = 48;

/// A packet of its own, freed when dropped.
struct Packet(*mut ff::AVPacket);

// A packet is plain data FFmpeg reference-counts; it moves to the thread
// that decodes it and is used there alone.
unsafe impl Send for Packet {}

impl Drop for Packet {
    fn drop(&mut self) {
        // SAFETY: allocated by FFmpeg, freed once.
        unsafe { ff::av_packet_free(&mut self.0) }
    }
}

/// Hands each decoder its packets, the streams side by side on threads of
/// their own when more than one has any (decoding is most of the work; a
/// stream's packets still go to its decoder in order, so what comes out is
/// what one thread makes). At the file's `end`, each is told so after.
fn decode_batch(decoders: &mut [AudioDecoder], batch: Vec<Vec<Packet>>, end: bool) {
    let decode = |decoder: &mut AudioDecoder, packets: Vec<Packet>| {
        for packet in packets {
            // SAFETY: the packet is valid and of this decoder's stream.
            if let Err(err) = unsafe { decoder.feed(packet.0) } {
                // A broken packet: left out, as a straight read leaves it.
                tracing::debug!("{}", err.report());
            }
        }
        if end {
            // SAFETY: a null packet asks the decoder for what it holds back.
            if let Err(err) = unsafe { decoder.feed(ptr::null()) } {
                tracing::warn!("{}", err.report());
            }
        }
    };
    let busy = batch.iter().filter(|b| !b.is_empty()).count();
    if busy < 2 {
        for (decoder, packets) in decoders.iter_mut().zip(batch) {
            decode(decoder, packets);
        }
        return;
    }
    let mut waiting: Vec<Option<Vec<Packet>>> = batch.into_iter().map(Some).collect();
    std::thread::scope(|scope| {
        for (decoder, slot) in decoders.iter_mut().zip(waiting.iter_mut()) {
            let spawned = std::thread::Builder::new()
                .name("sound decoding".into())
                .spawn_scoped(scope, || {
                    if let Some(packets) = slot.take() {
                        decode(decoder, packets);
                    }
                });
            if let Err(err) = spawned {
                tracing::warn!("a sound stream could not be decoded on a thread of its own: {err}");
            }
        }
    });
    // A stream no thread could be started for: here, after the others.
    for (decoder, slot) in decoders.iter_mut().zip(waiting) {
        if let Some(packets) = slot {
            decode(decoder, packets);
        }
    }
}

impl Drop for AudioStreams {
    fn drop(&mut self) {
        // SAFETY: allocated by FFmpeg, freed once.
        unsafe { ff::av_packet_free(&mut self.packet) }
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

    /// Every sample of `stream`, then a few seeks, read back.
    fn read_through(path: &Path, stream: u32) -> (Vec<f32>, Vec<Vec<f32>>, f64, f64) {
        let mut d = AudioDecoder::open(path, stream, 48_000).unwrap();
        let mut all = Vec::new();
        let mut block = vec![0.0f32; 4096];
        loop {
            let got = d.read(&mut block);
            if got == 0 {
                break;
            }
            all.extend_from_slice(&block[..got * CHANNELS]);
        }
        let seeks = [0.0, 0.42, 1.7, 0.05]
            .iter()
            .map(|&at| {
                d.seek(at);
                let mut part = vec![0.0f32; 9600];
                let got = d.read(&mut part);
                part.truncate(got * CHANNELS);
                part
            })
            .collect();
        (all, seeks, d.origin, d.file_start)
    }

    #[test]
    fn a_file_opened_again_from_its_header_reads_the_same_samples() {
        // An MP3's header doesn't say its rate: it is probed each time.
        for (name, streams, from_header) in [
            ("game-3-audio.mkv", &[1u32, 2, 3][..], true),
            ("sound-offset.mkv", &[1, 2], true),
            ("five-streams.mkv", &[1, 5], true),
            ("voice.mp3", &[0], false),
            ("x264-default-20s.mp4", &[1], true),
            ("phone-rotated.mp4", &[1], true),
        ] {
            let path = fixture(name);
            for &stream in streams {
                // Probed (kept), then from the header with what was kept.
                PROBED.lock().unwrap().take();
                let probed = read_through(&path, stream);
                assert!(
                    PROBED
                        .lock()
                        .unwrap()
                        .as_ref()
                        .is_some_and(|p| p.contains_key(&path))
                );
                let before = FROM_HEADER.load(std::sync::atomic::Ordering::Relaxed);
                let header = read_through(&path, stream);
                if from_header {
                    assert!(
                        FROM_HEADER.load(std::sync::atomic::Ordering::Relaxed) > before,
                        "{name}: from the header"
                    );
                }
                assert!(!probed.0.is_empty(), "{name} {stream} has sound");
                assert!(probed == header, "{name} stream {stream} reads the same");
            }
        }
    }

    #[test]
    fn a_seek_searches_by_the_picture_when_the_file_has_one() {
        let recording = AudioDecoder::open(&fixture("game-3-audio.mkv"), 2, 48_000).unwrap();
        assert_eq!((recording.stream, recording.seek_stream), (2, 0));
        let sound_only = AudioDecoder::open(&fixture("voice.mp3"), 0, 48_000).unwrap();
        assert_eq!((sound_only.stream, sound_only.seek_stream), (0, 0));
    }

    #[test]
    fn time_0_is_the_pictures_start() {
        // Picture from 0.1 s, the sound streams from 0 and 0.35 s.
        let early = AudioDecoder::open(&fixture("sound-offset.mkv"), 1, 48_000).unwrap();
        assert!((early.origin - 0.1).abs() < 1e-9, "{}", early.origin);
        let sound_only = AudioDecoder::open(&fixture("voice.mp3"), 0, 48_000).unwrap();
        assert!(
            sound_only.origin.abs() < 0.1,
            "its own start: {}",
            sound_only.origin
        );
    }

    #[test]
    fn the_grid_is_learnt_from_the_first_two_frames() {
        // Opus: 20 ms frames, the first shortened by the encoder's priming.
        let opus = AudioDecoder::open(&fixture("game-3-audio.mkv"), 1, 48_000).unwrap();
        assert_eq!(
            opus.grid,
            Grid::Known {
                first: 648,
                size: 960
            }
        );
        // Read at another rate, timestamps are taken as they are.
        let resampled = AudioDecoder::open(&fixture("game-3-audio.mkv"), 1, 44_100).unwrap();
        assert_eq!(resampled.grid, Grid::Unknown);
    }

    #[test]
    fn rounded_timestamps_go_back_to_the_grid() {
        let grid = Grid::Known {
            first: 648,
            size: 960,
        };
        // 34 ms rounds 33.5 ms (sample 1608): moved back to it.
        assert_eq!(snap(grid, 1632, 48), 1608);
        assert_eq!(snap(grid, 1584, 48), 1608);
        // Further than rounding: a frame of another length; kept.
        assert_eq!(snap(grid, 1608 + 300, 48), 1908);
        assert_eq!(snap(Grid::Unknown, 1632, 48), 1632);
        // A hole of whole frames stays on the grid.
        assert_eq!(snap(grid, 648 + 960 * 1000 + 20, 48), 648 + 960 * 1000);
    }

    #[test]
    fn sound_is_placed_at_the_end_of_the_queue() {
        assert_eq!(place(480_000, 479_000, 960), Placed::Before);
        assert_eq!(place(480_000, 480_000, 960), Placed::Across { drop: 0 });
        assert_eq!(place(480_000, 479_520, 960), Placed::Across { drop: 480 });
        assert_eq!(place(480_000, 480_528, 960), Placed::After { silence: 528 });
        assert_eq!(
            place(480_000, 3_360_000, 960),
            Placed::After { silence: 2_880_000 }
        );
    }
}
