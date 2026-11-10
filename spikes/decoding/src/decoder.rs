//! A thin FFmpeg video decoder: software, or hardware with frames copied back
//! to memory. Spike code: errors are plain strings.

use ffmpeg_sys_next as ff;
use std::ffi::{CStr, CString};
use std::ptr;

pub type Result<T> = std::result::Result<T, String>;

/// How frames are decoded.
#[derive(Clone, Debug)]
pub enum Mode {
    /// On the CPU, all cores.
    Software,
    /// On the GPU's video decoder through this FFmpeg device type
    /// (d3d11va, d3d12va, vaapi, vulkan, ...), copied back to memory.
    HardwareCopy(String),
    /// Vulkan Video on wgpu's own device; frames stay on the GPU.
    VulkanDirect,
}

/// One decoded frame in memory: planes and their row lengths.
pub struct Planes<'a> {
    pub format: PlaneFormat,
    pub width: u32,
    pub height: u32,
    /// (data, bytes per row) for each plane.
    pub planes: Vec<(&'a [u8], u32)>,
    /// Presentation time in seconds.
    pub time: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PlaneFormat {
    /// Luma plus interleaved chroma at half size (what hardware decoders give).
    Nv12,
    /// Three planes, chroma at half size (what software decoders give).
    Yuv420p,
    /// Still on the GPU, ready to sample (no planes in memory).
    OnGpu,
}

pub struct Decoder {
    format: *mut ff::AVFormatContext,
    codec: *mut ff::AVCodecContext,
    stream: i32,
    time_base: f64,
    packet: *mut ff::AVPacket,
    frame: *mut ff::AVFrame,
    copy: *mut ff::AVFrame,
    hardware: bool,
    direct: Option<crate::vulkan_direct::Shared>,
    draining: bool,
    pub codec_name: String,
    pub duration: f64,
}

// The decoder is used by one thread at a time.
unsafe impl Send for Decoder {}

pub fn describe(code: i32, what: &str) -> String {
    let mut buf = [0 as std::ffi::c_char; 256];
    unsafe { ff::av_strerror(code, buf.as_mut_ptr(), buf.len()) };
    let text = unsafe { CStr::from_ptr(buf.as_ptr()) }.to_string_lossy();
    format!("{what}: {text}")
}

/// Picks the hardware pixel format FFmpeg offers, stored in `opaque`.
unsafe extern "C" fn pick_hw_format(
    ctx: *mut ff::AVCodecContext,
    offered: *const ff::AVPixelFormat,
) -> ff::AVPixelFormat {
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

impl Decoder {
    pub fn open(path: &str, mode: &Mode) -> Result<Decoder> {
        unsafe {
            let cpath = CString::new(path).map_err(|e| e.to_string())?;
            let mut format = ptr::null_mut();
            let r = ff::avformat_open_input(&mut format, cpath.as_ptr(), ptr::null(), ptr::null_mut());
            if r < 0 {
                return Err(describe(r, "open"));
            }
            let r = ff::avformat_find_stream_info(format, ptr::null_mut());
            if r < 0 {
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
                return Err(describe(stream, "no video stream"));
            }
            let st = *(*format).streams.add(stream as usize);
            let tb = (*st).time_base;
            let codec = ff::avcodec_alloc_context3(decoder);
            let r = ff::avcodec_parameters_to_context(codec, (*st).codecpar);
            if r < 0 {
                return Err(describe(r, "codec parameters"));
            }
            let mut direct = None;
            let hardware = match mode {
                Mode::Software => {
                    (*codec).thread_count = 0;
                    (*codec).thread_type = (ff::FF_THREAD_FRAME | ff::FF_THREAD_SLICE) as i32;
                    false
                }
                Mode::HardwareCopy(name) => {
                    let cname = CString::new(name.as_str()).map_err(|e| e.to_string())?;
                    let kind = ff::av_hwdevice_find_type_by_name(cname.as_ptr());
                    if kind == ff::AVHWDeviceType::AV_HWDEVICE_TYPE_NONE {
                        return Err(format!("unknown hardware type {name}"));
                    }
                    let mut pix = None;
                    for i in 0.. {
                        let cfg = ff::avcodec_get_hw_config(decoder, i);
                        if cfg.is_null() {
                            break;
                        }
                        if (*cfg).device_type == kind
                            && (*cfg).methods & ff::AV_CODEC_HW_CONFIG_METHOD_HW_DEVICE_CTX as i32 != 0
                        {
                            pix = Some((*cfg).pix_fmt);
                            break;
                        }
                    }
                    let pix = pix.ok_or_else(|| format!("{name} can't decode this codec"))?;
                    let mut device = ptr::null_mut();
                    let r = ff::av_hwdevice_ctx_create(&mut device, kind, ptr::null(), ptr::null_mut(), 0);
                    if r < 0 {
                        return Err(describe(r, &format!("create {name} device")));
                    }
                    (*codec).hw_device_ctx = device; // the context takes this reference
                    (*codec).opaque = pix as i32 as isize as *mut std::ffi::c_void;
                    (*codec).get_format = Some(pick_hw_format);
                    true
                }
                Mode::VulkanDirect => {
                    let (shared, device) = crate::vulkan_direct::Shared::open()?;
                    (*codec).hw_device_ctx = device;
                    (*codec).opaque =
                        ff::AVPixelFormat::AV_PIX_FMT_VULKAN as i32 as isize as *mut std::ffi::c_void;
                    (*codec).get_format = Some(pick_hw_format);
                    direct = Some(shared);
                    false
                }
            };
            let r = ff::avcodec_open2(codec, decoder, ptr::null_mut());
            if r < 0 {
                return Err(describe(r, "open codec"));
            }
            let codec_name = CStr::from_ptr((*decoder).name).to_string_lossy().into_owned();
            let duration = if (*format).duration > 0 {
                (*format).duration as f64 / ff::AV_TIME_BASE as f64
            } else {
                0.0
            };
            Ok(Decoder {
                format,
                codec,
                stream,
                time_base: tb.num as f64 / tb.den as f64,
                packet: ff::av_packet_alloc(),
                frame: ff::av_frame_alloc(),
                copy: ff::av_frame_alloc(),
                hardware,
                direct,
                draining: false,
                codec_name,
                duration,
            })
        }
    }

    /// Which GPU and driver the shared Vulkan device uses, for the results table.
    pub fn device_note(&self) -> Option<&str> {
        self.direct.as_ref().map(|d| d.description.as_str())
    }

    /// Decodes the next frame, or `None` at the end.
    pub fn next(&mut self) -> Result<Option<Planes<'_>>> {
        unsafe {
            loop {
                ff::av_frame_unref(self.frame);
                let r = ff::avcodec_receive_frame(self.codec, self.frame);
                if r == 0 {
                    return self.planes().map(Some);
                }
                if r == ff::AVERROR_EOF {
                    return Ok(None);
                }
                if r != ff::AVERROR(ff::EAGAIN) {
                    return Err(describe(r, "decode"));
                }
                if self.draining {
                    return Ok(None);
                }
                // Feed one more packet of our stream.
                loop {
                    let r = ff::av_read_frame(self.format, self.packet);
                    if r < 0 {
                        ff::avcodec_send_packet(self.codec, ptr::null());
                        self.draining = true;
                        break;
                    }
                    let ours = (*self.packet).stream_index == self.stream;
                    if ours {
                        let r = ff::avcodec_send_packet(self.codec, self.packet);
                        ff::av_packet_unref(self.packet);
                        if r < 0 && r != ff::AVERROR(ff::EAGAIN) {
                            return Err(describe(r, "send packet"));
                        }
                        break;
                    }
                    ff::av_packet_unref(self.packet);
                }
            }
        }
    }

    unsafe fn planes(&mut self) -> Result<Planes<'_>> {
        unsafe {
            let ts = (*self.frame).best_effort_timestamp;
            let time = ts as f64 * self.time_base;
            let mut f = self.frame;
            if let Some(shared) = &self.direct {
                if (*f).format != ff::AVPixelFormat::AV_PIX_FMT_VULKAN as i32 {
                    return Err("FFmpeg fell back to software decoding".into());
                }
                shared.wait_for(f)?;
                return Ok(Planes {
                    format: PlaneFormat::OnGpu,
                    width: (*f).width as u32,
                    height: (*f).height as u32,
                    planes: Vec::new(),
                    time,
                });
            }
            if self.hardware {
                ff::av_frame_unref(self.copy);
                let r = ff::av_hwframe_transfer_data(self.copy, self.frame, 0);
                if r < 0 {
                    return Err(describe(r, "copy from GPU"));
                }
                f = self.copy;
            }
            let format = match (*f).format {
                x if x == ff::AVPixelFormat::AV_PIX_FMT_NV12 as i32 => PlaneFormat::Nv12,
                x if x == ff::AVPixelFormat::AV_PIX_FMT_YUV420P as i32 => PlaneFormat::Yuv420p,
                x => {
                    let name = ff::av_get_pix_fmt_name(std::mem::transmute::<i32, ff::AVPixelFormat>(x));
                    let name = if name.is_null() {
                        "?".into()
                    } else {
                        CStr::from_ptr(name).to_string_lossy()
                    };
                    return Err(format!("frame format {name} not handled by the spike"));
                }
            };
            let (w, h) = ((*f).width as u32, (*f).height as u32);
            let rows = |plane: usize| if plane == 0 { h } else { h.div_ceil(2) };
            let count = if format == PlaneFormat::Nv12 { 2 } else { 3 };
            let planes = (0..count)
                .map(|i| {
                    let stride = (*f).linesize[i] as u32;
                    let len = (stride * rows(i)) as usize;
                    (std::slice::from_raw_parts((*f).data[i], len), stride)
                })
                .collect();
            Ok(Planes {
                format,
                width: w,
                height: h,
                planes,
                time,
            })
        }
    }

    /// Jumps to the frame at `seconds`: back to the key frame before it, then
    /// decodes forward. Returns the time of the frame reached.
    pub fn seek_and_decode(&mut self, seconds: f64, mut on_frame: impl FnMut(&Planes)) -> Result<f64> {
        unsafe {
            let ts = (seconds / self.time_base) as i64;
            let r = ff::av_seek_frame(self.format, self.stream, ts, ff::AVSEEK_FLAG_BACKWARD as i32);
            if r < 0 {
                return Err(describe(r, "seek"));
            }
            ff::avcodec_flush_buffers(self.codec);
            self.draining = false;
        }
        loop {
            match self.next()? {
                Some(p) => {
                    if p.time + 1e-6 >= seconds {
                        let t = p.time;
                        on_frame(&p);
                        return Ok(t);
                    }
                }
                None => return Ok(seconds),
            }
        }
    }
}

impl Drop for Decoder {
    fn drop(&mut self) {
        unsafe {
            ff::av_frame_free(&mut self.frame);
            ff::av_frame_free(&mut self.copy);
            ff::av_packet_free(&mut self.packet);
            ff::avcodec_free_context(&mut self.codec);
            ff::avformat_close_input(&mut self.format);
        }
    }
}
