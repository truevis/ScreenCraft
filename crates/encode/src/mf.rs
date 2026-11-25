//! H.264 on Windows through Media Foundation (the operating system's licensed encoder and
//! decoder). Synchronous transforms: NV12 in / Annex-B out for encoding, the reverse for decoding.
//!

#![allow(non_snake_case)]

use crate::codec::VideoEncoder;
use crate::h264::{annexb_to_avcc, avcc_to_annexb, build_avcc};
use crate::types::*;
use crate::{Error, Result};
use std::collections::VecDeque;
use std::mem::ManuallyDrop;
use std::sync::OnceLock;
use windows::core::Interface;
use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED};

/// COM is per thread: the encoder is built on the recorder's thread and then driven from the
/// encode thread, so every thread that touches a transform joins the multithreaded apartment.
/// Media Foundation itself is started once for the process and never shut down.
fn init() -> Result<()> {
    thread_local! {
        static COM: () = unsafe {
            // RPC_E_CHANGED_MODE (an STA thread) is fine: the MFTs used here are free-threaded.
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        };
    }
    COM.with(|_| ());
    static STARTED: OnceLock<Option<String>> = OnceLock::new();
    let failed = STARTED.get_or_init(|| unsafe { MFStartup(MF_VERSION, MFSTARTUP_FULL).err().map(|e| format!("Media Foundation failed to start: {e}")) });
    match failed {
        Some(m) => Err(Error::Codec(m.clone())),
        None => Ok(()),
    }
}

fn err(what: &str, e: windows::core::Error) -> Error {
    Error::Codec(format!("{what}: {e}"))
}

fn pack(hi: u32, lo: u32) -> u64 {
    ((hi as u64) << 32) | lo as u64
}

unsafe fn video_type(subtype: &windows::core::GUID, w: u32, h: u32, fps: (u32, u32)) -> windows::core::Result<IMFMediaType> {
    let t = MFCreateMediaType()?;
    t.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
    t.SetGUID(&MF_MT_SUBTYPE, subtype)?;
    t.SetUINT64(&MF_MT_FRAME_SIZE, pack(w, h))?;
    t.SetUINT64(&MF_MT_FRAME_RATE, pack(fps.0, fps.1))?;
    t.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, pack(1, 1))?;
    t.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
    Ok(t)
}

unsafe fn sample_from(bytes: &[u8], pts_100ns: i64, dur_100ns: i64) -> windows::core::Result<IMFSample> {
    let buf = MFCreateMemoryBuffer(bytes.len() as u32)?;
    let mut p = std::ptr::null_mut();
    buf.Lock(&mut p, None, None)?;
    std::ptr::copy_nonoverlapping(bytes.as_ptr(), p, bytes.len());
    buf.Unlock()?;
    buf.SetCurrentLength(bytes.len() as u32)?;
    let s = MFCreateSample()?;
    s.AddBuffer(&buf)?;
    s.SetSampleTime(pts_100ns)?;
    s.SetSampleDuration(dur_100ns)?;
    Ok(s)
}

unsafe fn sample_bytes(s: &IMFSample) -> windows::core::Result<Vec<u8>> {
    let buf = s.ConvertToContiguousBuffer()?;
    let (mut p, mut len) = (std::ptr::null_mut(), 0u32);
    buf.Lock(&mut p, None, Some(&mut len))?;
    let v = std::slice::from_raw_parts(p, len as usize).to_vec();
    buf.Unlock()?;
    Ok(v)
}

/// Pull every finished output sample out of `mft`.
unsafe fn drain(mft: &IMFTransform, provides_samples: bool, out_size: u32) -> windows::core::Result<Vec<IMFSample>> {
    let mut out = Vec::new();
    loop {
        let sample = if provides_samples {
            None
        } else {
            let s = MFCreateSample()?;
            s.AddBuffer(&MFCreateMemoryBuffer(out_size)?)?;
            Some(s)
        };
        let mut buf = [MFT_OUTPUT_DATA_BUFFER { dwStreamID: 0, pSample: ManuallyDrop::new(sample), dwStatus: 0, pEvents: ManuallyDrop::new(None) }];
        let mut status = 0u32;
        match mft.ProcessOutput(0, &mut buf, &mut status) {
            Ok(()) => {
                if let Some(s) = ManuallyDrop::into_inner(std::mem::replace(&mut buf[0].pSample, ManuallyDrop::new(None))) {
                    out.push(s);
                }
                let _ = ManuallyDrop::into_inner(std::mem::replace(&mut buf[0].pEvents, ManuallyDrop::new(None)));
            }
            Err(e) => {
                // ProcessOutput fails without releasing the sample we handed it.
                let _ = ManuallyDrop::into_inner(std::mem::replace(&mut buf[0].pSample, ManuallyDrop::new(None)));
                let _ = ManuallyDrop::into_inner(std::mem::replace(&mut buf[0].pEvents, ManuallyDrop::new(None)));
                if e.code() == MF_E_TRANSFORM_NEED_MORE_INPUT {
                    break;
                }
                return Err(e);
            }
        }
    }
    Ok(out)
}

/// A compressed frame never needs more room than the raw NV12 picture (plus slack).
fn frame_bytes(cfg: &VideoConfig) -> u32 {
    (cfg.width * cfg.height * 3 / 2).max(1 << 20)
}

pub struct MfH264Encoder {
    mft: IMFTransform,
    cfg: VideoConfig,
    avcc: Vec<u8>,
    provides: bool,
    out_size: u32,
    /// Presentation times of frames still inside the encoder, in submission order.
    /// Media Foundation's sample times are not monotonic once B-frames are enabled, and the
    /// muxer rejects a decreasing timestamp before it can write `moov`.
    pending_pts: VecDeque<i64>,
}

// The transform is only used through &mut self from one thread at a time.
unsafe impl Send for MfH264Encoder {}

impl MfH264Encoder {
    pub fn new(cfg: &VideoConfig) -> Result<Self> {
        init()?;
        unsafe {
            let mft: IMFTransform = CoCreateInstance(&CLSID_MSH264EncoderMFT, None, CLSCTX_INPROC_SERVER).map_err(|e| err("no H.264 encoder", e))?;
            // Low latency disables frame reordering, so output order matches the pts queue.
            if let Ok(attrs) = mft.GetAttributes() {
                let _ = attrs.SetUINT32(&MF_LOW_LATENCY, 1);
            }
            let fps = (cfg.fps_num, cfg.fps_den.max(1));
            let out_t = video_type(&MFVideoFormat_H264, cfg.width, cfg.height, fps).map_err(|e| err("encoder type", e))?;
            out_t.SetUINT32(&MF_MT_AVG_BITRATE, cfg.bitrate_kbps.saturating_mul(1000)).map_err(|e| err("bitrate", e))?;
            out_t.SetUINT32(&MF_MT_MPEG2_PROFILE, 100).map_err(|e| err("profile", e))?;
            // Short GOPs keep seeking in the editor fast; the encoder treats this as a ceiling.
            let _ = out_t.SetUINT32(&MF_MT_MAX_KEYFRAME_SPACING, cfg.effective_keyint().max(1));
            mft.SetOutputType(0, &out_t, 0).map_err(|e| err("encoder output type", e))?;
            let in_t = video_type(&MFVideoFormat_NV12, cfg.width, cfg.height, fps).map_err(|e| err("encoder input type", e))?;
            mft.SetInputType(0, &in_t, 0).map_err(|e| err("encoder input type", e))?;
            mft.ProcessMessage(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0).map_err(|e| err("start", e))?;
            mft.ProcessMessage(MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0).map_err(|e| err("start", e))?;
            let info = mft.GetOutputStreamInfo(0).map_err(|e| err("stream info", e))?;
            let provides = info.dwFlags & MFT_OUTPUT_STREAM_PROVIDES_SAMPLES.0 as u32 != 0;
            Ok(Self { mft, cfg: cfg.clone(), avcc: Vec::new(), provides, out_size: info.cbSize.max(frame_bytes(cfg)), pending_pts: VecDeque::new() })
        }
    }

    /// The encoder signals a new output type (often once SPS/PPS exist, and again on drain).
    unsafe fn renegotiate(&mut self) -> Result<()> {
        let t = self.mft.GetOutputAvailableType(0, 0).map_err(|e| err("encoder output type", e))?;
        self.mft.SetOutputType(0, &t, 0).map_err(|e| err("encoder output type", e))?;
        let info = self.mft.GetOutputStreamInfo(0).map_err(|e| err("stream info", e))?;
        self.provides = info.dwFlags & MFT_OUTPUT_STREAM_PROVIDES_SAMPLES.0 as u32 != 0;
        self.out_size = info.cbSize.max(frame_bytes(&self.cfg));
        Ok(())
    }

    unsafe fn drain_encoder(&mut self) -> Result<Vec<IMFSample>> {
        loop {
            match drain(&self.mft, self.provides, self.out_size) {
                Ok(samples) => return Ok(samples),
                Err(e) if e.code() == MF_E_TRANSFORM_STREAM_CHANGE => self.renegotiate()?,
                Err(e) => return Err(err("encode output", e)),
            }
        }
    }

    fn collect(&mut self, samples: Vec<IMFSample>) -> Result<Vec<Packet>> {
        let mut out = Vec::new();
        for s in samples {
            let bytes = unsafe { sample_bytes(&s).map_err(|e| err("read output", e))? };
            let (data, ps, key) = annexb_to_avcc(&bytes);
            if let Some((sps, pps)) = ps {
                if self.avcc.is_empty() {
                    self.avcc = build_avcc(&sps, &pps);
                }
            }
            if !data.is_empty() {
                let pts = self.pending_pts.pop_front().unwrap_or(0);
                out.push(Packet { data, pts_us: pts, key });
            }
        }
        Ok(out)
    }
}

impl VideoEncoder for MfH264Encoder {
    fn name(&self) -> &'static str {
        "Media Foundation"
    }

    fn extradata(&self) -> Vec<u8> {
        self.avcc.clone()
    }

    fn encode(&mut self, f: &I420, pts_us: i64) -> Result<Vec<Packet>> {
        init()?;
        if f.width != self.cfg.width || f.height != self.cfg.height {
            return Err(Error::Config("frame size changed mid-stream".into()));
        }
        let (w, h) = (f.width as usize, f.height as usize);
        // I420 -> NV12
        let mut nv = Vec::with_capacity(w * h * 3 / 2);
        nv.extend_from_slice(&f.y);
        for i in 0..(w / 2) * (h / 2) {
            nv.push(f.u[i]);
            nv.push(f.v[i]);
        }
        let dur = self.cfg.frame_duration_us() * 10;
        self.pending_pts.push_back(pts_us);
        let samples = unsafe {
            let s = sample_from(&nv, pts_us * 10, dur).map_err(|e| err("input sample", e))?;
            let mut early = Vec::new();
            // A transform that still holds output refuses input until it is drained.
            let mut tries = 0;
            loop {
                match self.mft.ProcessInput(0, &s, 0) {
                    Ok(()) => break,
                    Err(e) if e.code() == MF_E_NOTACCEPTING && tries < 8 => {
                        tries += 1;
                        early.extend(self.drain_encoder()?);
                    }
                    Err(e) => {
                        self.pending_pts.pop_back();
                        return Err(err("encode", e));
                    }
                }
            }
            early.extend(self.drain_encoder()?);
            early
        };
        self.collect(samples)
    }

    fn flush(&mut self) -> Result<Vec<Packet>> {
        init()?;
        let samples = unsafe {
            self.mft.ProcessMessage(MFT_MESSAGE_NOTIFY_END_OF_STREAM, 0).map_err(|e| err("flush", e))?;
            self.mft.ProcessMessage(MFT_MESSAGE_COMMAND_DRAIN, 0).map_err(|e| err("flush", e))?;
            self.drain_encoder().map_err(|e| match e {
                Error::Codec(m) if m.starts_with("encode output") => Error::Codec(m.replacen("encode output", "flush output", 1)),
                other => other,
            })?
        };
        self.collect(samples)
    }
}

pub struct MfH264Decoder {
    mft: IMFTransform,
    params: Vec<Vec<u8>>,
    length_size: usize,
    provides: bool,
    out_size: u32,
    width: u32,
    height: u32,
    /// Rows in the decoded buffer: the coded height, padded to 16 (1080 → 1088).
    buf_rows: u32,
    stride: u32,
    t: i64,
}

unsafe impl Send for MfH264Decoder {}

impl MfH264Decoder {
    pub fn new(params: &[Vec<u8>], length_size: usize) -> Result<Self> {
        init()?;
        unsafe {
            let mft: IMFTransform = CoCreateInstance(&CLSID_MSH264DecoderMFT, None, CLSCTX_INPROC_SERVER).map_err(|e| err("no H.264 decoder", e))?;
            // Without this the decoder holds several pictures back; the editor needs one
            // picture out for each sample in (recordings have no B-frames).
            if let Ok(attrs) = mft.GetAttributes() {
                let _ = attrs.SetUINT32(&MF_LOW_LATENCY, 1);
            }
            let in_t = MFCreateMediaType().map_err(|e| err("decoder type", e))?;
            in_t.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video).map_err(|e| err("decoder type", e))?;
            in_t.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_H264).map_err(|e| err("decoder type", e))?;
            mft.SetInputType(0, &in_t, 0).map_err(|e| err("decoder input type", e))?;
            let mut d = Self { mft, params: params.to_vec(), length_size, provides: false, out_size: 0, width: 0, height: 0, buf_rows: 0, stride: 0, t: 0 };
            d.pick_output()?;
            d.mft.ProcessMessage(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0).map_err(|e| err("start", e))?;
            d.mft.ProcessMessage(MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0).map_err(|e| err("start", e))?;
            Ok(d)
        }
    }

    /// Choose NV12 output and read its size.
    unsafe fn pick_output(&mut self) -> Result<()> {
        let mut i = 0;
        loop {
            let t = self.mft.GetOutputAvailableType(0, i).map_err(|e| err("no NV12 output", e))?;
            if t.GetGUID(&MF_MT_SUBTYPE).map(|g| g == MFVideoFormat_NV12).unwrap_or(false) {
                self.mft.SetOutputType(0, &t, 0).map_err(|e| err("decoder output type", e))?;
                let fs = t.GetUINT64(&MF_MT_FRAME_SIZE).unwrap_or(0);
                self.width = (fs >> 32) as u32;
                self.buf_rows = fs as u32;
                self.height = self.buf_rows;
                // The visible picture (e.g. 1920x1080 inside a 1920x1088 buffer).
                let mut area = [0u8; 16];
                let mut got = 0u32;
                if t.GetBlob(&MF_MT_MINIMUM_DISPLAY_APERTURE, &mut area, Some(&mut got)).is_ok() && got as usize >= 16 {
                    let cx = i32::from_le_bytes([area[8], area[9], area[10], area[11]]);
                    let cy = i32::from_le_bytes([area[12], area[13], area[14], area[15]]);
                    if cx > 0 && cy > 0 && cx as u32 <= self.width && cy as u32 <= self.buf_rows {
                        self.width = cx as u32;
                        self.height = cy as u32;
                    }
                }
                self.stride = t.GetUINT32(&MF_MT_DEFAULT_STRIDE).unwrap_or(fs.wrapping_shr(32) as u32);
                let info = self.mft.GetOutputStreamInfo(0).map_err(|e| err("stream info", e))?;
                self.provides = info.dwFlags & MFT_OUTPUT_STREAM_PROVIDES_SAMPLES.0 as u32 != 0;
                self.out_size = info.cbSize.max(self.stride.max(self.width) * self.buf_rows * 3 / 2);
                return Ok(());
            }
            i += 1;
        }
    }

    /// Feed one length-prefixed access unit; returns the picture when one is ready.
    pub fn decode(&mut self, sample: &[u8]) -> Result<Option<I420>> {
        init()?;
        let key = sample.len() > self.length_size && {
            // an IDR slice carries the parameter sets in front of it for the decoder
            let mut i = 0;
            let mut found = false;
            while i + self.length_size < sample.len() {
                let mut n = 0usize;
                for k in 0..self.length_size {
                    n = (n << 8) | sample[i + k] as usize;
                }
                i += self.length_size;
                if i < sample.len() && sample[i] & 0x1F == 5 {
                    found = true;
                    break;
                }
                i += n;
            }
            found
        };
        let annexb = avcc_to_annexb(sample, self.length_size, if key { Some(&self.params) } else { None });
        unsafe {
            let s = sample_from(&annexb, self.t, 333_333).map_err(|e| err("input sample", e))?;
            self.t += 333_333;
            let mut pics = Vec::new();
            // A decoder that still holds output refuses input until it is drained.
            let mut tries = 0;
            loop {
                match self.mft.ProcessInput(0, &s, 0) {
                    Ok(()) => break,
                    Err(e) if e.code() == MF_E_NOTACCEPTING && tries < 8 => {
                        tries += 1;
                        pics.extend(self.drain_pictures()?);
                    }
                    Err(e) => return Err(err("decode", e)),
                }
            }
            pics.extend(self.drain_pictures()?);
            if pics.is_empty() {
                // Still buffered: force it out. Drain keeps reference pictures, so decoding
                // simply continues with the next sample.
                self.mft.ProcessMessage(MFT_MESSAGE_COMMAND_DRAIN, 0).map_err(|e| err("drain", e))?;
                pics.extend(self.drain_pictures()?);
            }
            Ok(pics.pop())
        }
    }

    unsafe fn drain_pictures(&mut self) -> Result<Vec<I420>> {
        loop {
            match drain(&self.mft, self.provides, self.out_size) {
                Ok(v) => {
                    let mut out = Vec::with_capacity(v.len());
                    for s in v {
                        let nv = sample_bytes(&s).map_err(|e| err("read picture", e))?;
                        out.push(self.to_i420(&nv));
                    }
                    return Ok(out);
                }
                Err(e) if e.code() == MF_E_TRANSFORM_STREAM_CHANGE => self.pick_output()?,
                Err(e) => return Err(err("decode output", e)),
            }
        }
    }

    fn to_i420(&self, nv: &[u8]) -> I420 {
        let (w, h) = (self.width as usize, self.height as usize);
        let st = (self.stride as usize).max(w);
        let rows = (self.buf_rows as usize).max(h);
        let mut f = I420::new(self.width & !1, self.height & !1);
        let (fw, fh) = (f.width as usize, f.height as usize);
        let (cw, ch) = (fw / 2, fh / 2);
        if nv.len() < st * rows + st * (rows / 2) {
            return f; // short buffer: hand back a blank picture rather than panic
        }
        for r in 0..fh {
            f.y[r * fw..r * fw + fw].copy_from_slice(&nv[r * st..r * st + fw]);
        }
        let uv = &nv[st * rows..];
        for r in 0..ch {
            for x in 0..cw {
                f.u[r * cw + x] = uv[r * st + x * 2];
                f.v[r * cw + x] = uv[r * st + x * 2 + 1];
            }
        }
        f
    }
}
