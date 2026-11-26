//! Decoding recordings back into frames and PCM for the editor and exporter.

use screencraft_encode::h264::H264Decoder;
use screencraft_encode::mp4read::{parse_avcc, Mp4File, SampleRef, Track, TrackKind};
use screencraft_encode::opus::OpusDecoder;
use screencraft_encode::I420;
use std::collections::VecDeque;
use std::path::Path;
use std::sync::Arc;

/// Presentation times of every frame of a recording, cheap to hold on the UI thread.
#[derive(Clone, Debug, Default)]
pub struct FrameTable {
    pts_ms: Vec<f64>,
    pub width: u32,
    pub height: u32,
}

impl FrameTable {
    pub fn open(path: &Path) -> Result<Self, String> {
        let file = Mp4File::open(path).map_err(|e| e.to_string())?;
        let t = file.video().ok_or("the file has no video track")?;
        Ok(Self { pts_ms: t.samples.iter().map(|s| s.pts_us as f64 / 1000.0).collect(), width: t.width, height: t.height })
    }

    pub fn open_project(package: &crate::Package, project: &screencraft_core::project::Project) -> Result<Self, String> {
        let mut pts_ms = Vec::new();
        let mut width = project.media.width;
        let mut height = project.media.height;
        for (idx, seg) in project.media.resolved_segments().iter().enumerate() {
            let file = Mp4File::open(&package.dir.join(&seg.screen)).map_err(|e| e.to_string())?;
            let t = file.video().ok_or("the file has no video track")?;
            if idx == 0 && (width == 0 || height == 0) {
                width = t.width;
                height = t.height;
            }
            for s in &t.samples {
                pts_ms.push(seg.offset_ms + s.pts_us as f64 / 1000.0);
            }
        }
        Ok(Self { pts_ms, width, height })
    }

    pub fn len(&self) -> usize {
        self.pts_ms.len()
    }

    pub fn is_empty(&self) -> bool {
        self.pts_ms.is_empty()
    }

    pub fn index_at(&self, t_ms: f64) -> usize {
        match self.pts_ms.binary_search_by(|x| x.partial_cmp(&t_ms).unwrap_or(std::cmp::Ordering::Equal)) {
            Ok(i) => i,
            Err(0) => 0,
            Err(i) => (i - 1).min(self.pts_ms.len().saturating_sub(1)),
        }
    }
}

/// Random-access H.264 video. Frames are decoded from the preceding key frame and the most
/// recent pictures are kept, so scrubbing back and forth stays cheap.
pub struct VideoSource {
    file: Arc<Mp4File>,
    track: usize,
    decoder: H264Decoder,
    length_size: usize,
    params: Vec<Vec<u8>>,
    /// Index of the sample the decoder produced last (so playback just continues).
    last_decoded: Option<usize>,
    cache: VecDeque<(usize, Arc<I420>)>,
    pub width: u32,
    pub height: u32,
    pub frame_count: usize,
    pub duration_ms: f64,
}

const CACHE_FRAMES: usize = 12;

impl VideoSource {
    pub fn open(path: &Path) -> Result<Self, String> {
        let file = Arc::new(Mp4File::open(path).map_err(|e| e.to_string())?);
        Self::from_file(file)
    }

    pub fn from_file(file: Arc<Mp4File>) -> Result<Self, String> {
        let track = file.tracks.iter().position(|t| t.kind == TrackKind::Video).ok_or("the file has no video track")?;
        let t: &Track = &file.tracks[track];
        if &t.codec != b"avc1" {
            return Err(format!("unsupported video codec '{}' (ScreenCraft edits H.264 recordings)", t.codec_str()));
        }
        let (length_size, params) = parse_avcc(&t.config).map_err(|e| e.to_string())?;
        let (width, height, frame_count) = (t.width, t.height, t.samples.len());
        let duration_ms = match (t.samples.first(), t.samples.last()) {
            (Some(a), Some(b)) if frame_count > 1 => {
                let step = (b.pts_us - a.pts_us) as f64 / (frame_count - 1) as f64;
                (b.pts_us as f64 + step) / 1000.0
            }
            _ => t.duration_us as f64 / 1000.0,
        };
        Ok(Self {
            file,
            track,
            decoder: H264Decoder::new(&params, length_size).map_err(|e| e.to_string())?,
            length_size,
            params,
            last_decoded: None,
            cache: VecDeque::new(),
            width,
            height,
            frame_count,
            duration_ms,
        })
    }

    fn samples(&self) -> &[SampleRef] {
        &self.file.tracks[self.track].samples
    }

    /// Index of the frame shown at `t_ms` (clamped).
    pub fn index_at(&self, t_ms: f64) -> usize {
        let s = self.samples();
        if s.is_empty() {
            return 0;
        }
        let t_us = (t_ms * 1000.0) as i64;
        match s.binary_search_by(|x| x.pts_us.cmp(&t_us)) {
            Ok(i) => i,
            Err(0) => 0,
            Err(i) => (i - 1).min(s.len() - 1),
        }
    }

    pub fn pts_ms(&self, idx: usize) -> f64 {
        self.samples().get(idx).map(|s| s.pts_us as f64 / 1000.0).unwrap_or(0.0)
    }

    pub fn fps(&self) -> f64 {
        if self.frame_count > 1 {
            (self.frame_count - 1) as f64 * 1000.0 / (self.pts_ms(self.frame_count - 1) - self.pts_ms(0)).max(1.0)
        } else {
            30.0
        }
    }

    pub fn frame_at(&mut self, t_ms: f64) -> Result<Arc<I420>, String> {
        let i = self.index_at(t_ms);
        self.frame(i)
    }

    pub fn frame(&mut self, idx: usize) -> Result<Arc<I420>, String> {
        let idx = idx.min(self.frame_count.saturating_sub(1));
        if let Some((_, f)) = self.cache.iter().find(|(i, _)| *i == idx) {
            return Ok(f.clone());
        }
        // Continue from the last decoded picture when the target is just ahead of it,
        // otherwise restart at the key frame at or before the target.
        let start = match self.last_decoded {
            Some(l) if l < idx && idx - l <= 90 && self.no_key_between(l + 1, idx) => l + 1,
            _ => {
                let k = self.keyframe_at_or_before(idx);
                if self.last_decoded.is_some() && self.decoder.flush().is_err() {
                    self.decoder = H264Decoder::new(&self.params, self.length_size).map_err(|e| e.to_string())?;
                }
                k
            }
        };
        let mut result = None;
        for i in start..=idx {
            let s = self.samples()[i].clone();
            let raw = self.file.read_sample(&s).map_err(|e| e.to_string())?;
            let pic = self.decoder.decode(&raw).map_err(|e| e.to_string())?;
            self.last_decoded = Some(i);
            if let Some(p) = pic {
                let arc = Arc::new(p);
                self.remember(i, arc.clone());
                if i == idx {
                    result = Some(arc);
                }
            }
        }
        match result {
            Some(r) => Ok(r),
            // The decoder skipped this picture (an encoder-skipped frame): show the previous one.
            None => self
                .cache
                .iter()
                .rev()
                .find(|(i, _)| *i <= idx)
                .map(|(_, f)| f.clone())
                .ok_or_else(|| "could not decode a frame".to_string()),
        }
    }

    fn remember(&mut self, idx: usize, f: Arc<I420>) {
        if self.cache.len() >= CACHE_FRAMES {
            self.cache.pop_front();
        }
        self.cache.push_back((idx, f));
    }

    fn keyframe_at_or_before(&self, idx: usize) -> usize {
        let s = self.samples();
        (0..=idx).rev().find(|&i| s[i].key).unwrap_or(0)
    }

    fn no_key_between(&self, a: usize, b: usize) -> bool {
        let s = self.samples();
        (a..=b).all(|i| !s[i].key || i == a)
    }
}

/// Opus audio track of a recording, decoded on demand.
pub struct AudioSource {
    file: Arc<Mp4File>,
    track: usize,
    decoder: OpusDecoder,
    pre_skip: usize,
    channels: usize,
    /// Next packet the decoder expects, so sequential reads need no pre-roll.
    next_packet: usize,
    pub duration_ms: f64,
}

const OPUS_PACKET: usize = 960;

impl AudioSource {
    /// Open the `n`-th audio track of the file.
    pub fn open(file: Arc<Mp4File>, audio_index: usize) -> Result<Self, String> {
        let track = file
            .tracks
            .iter()
            .enumerate()
            .filter(|(_, t)| t.kind == TrackKind::Audio)
            .nth(audio_index)
            .map(|(i, _)| i)
            .ok_or("no such audio track")?;
        let t = &file.tracks[track];
        if &t.codec != b"Opus" {
            return Err(format!("unsupported audio codec '{}'", t.codec_str()));
        }
        let channels = t.channels.clamp(1, 2) as usize;
        // dOps: version, channels, pre-skip (BE u16) ...
        let pre_skip = if t.config.len() >= 4 { u16::from_be_bytes([t.config[2], t.config[3]]) as usize } else { 312 };
        let duration_ms = t.samples.len() as f64 * 20.0;
        Ok(Self {
            decoder: OpusDecoder::new(channels as u8).map_err(|e| e.to_string())?,
            file,
            track,
            pre_skip,
            channels,
            next_packet: 0,
            duration_ms,
        })
    }

    /// 48 kHz stereo samples for `[start_ms, start_ms + len_frames/48)`. Past the end it
    /// returns silence.
    pub fn read(&mut self, start_ms: f64, len_frames: usize) -> Result<Vec<f32>, String> {
        let mut out = vec![0f32; len_frames * 2];
        let start_frame = (start_ms.max(0.0) * 48.0) as usize + self.pre_skip;
        let first_packet = start_frame / OPUS_PACKET;
        let n_packets = self.file.tracks[self.track].samples.len();
        // Sequential reads continue; anything else restarts with one packet of pre-roll.
        let mut p = if first_packet == self.next_packet {
            first_packet
        } else {
            self.decoder = OpusDecoder::new(self.channels as u8).map_err(|e| e.to_string())?;
            first_packet.saturating_sub(1)
        };
        let end_frame = start_frame + len_frames;
        while p * OPUS_PACKET < end_frame && p < n_packets {
            let s = self.file.tracks[self.track].samples[p].clone();
            let raw = self.file.read_sample(&s).map_err(|e| e.to_string())?;
            let pcm = self.decoder.decode(&raw).map_err(|e| e.to_string())?;
            self.next_packet = p + 1;
            let frames = pcm.len() / self.channels;
            for k in 0..frames {
                let abs = p * OPUS_PACKET + k;
                if abs < start_frame || abs >= end_frame {
                    continue;
                }
                let o = (abs - start_frame) * 2;
                if self.channels == 1 {
                    out[o] = pcm[k];
                    out[o + 1] = pcm[k];
                } else {
                    out[o] = pcm[k * 2];
                    out[o + 1] = pcm[k * 2 + 1];
                }
            }
            p += 1;
        }
        Ok(out)
    }
}

/// Everything the compositor needs for one output frame.
pub struct Frames {
    pub screen: Arc<I420>,
    /// The other clip's picture during a cross-dissolve.
    pub screen2: Option<Arc<I420>>,
    pub camera: Option<Arc<I420>>,
    pub camera2: Option<Arc<I420>>,
}

struct VideoSegment {
    path: std::path::PathBuf,
    offset_ms: f64,
    duration_ms: f64,
    primary: Option<VideoSource>,
    secondary: Option<VideoSource>,
}

/// The screen recording and (when there is one) the camera recording, decoded on demand. A second
/// decoder for each is opened only while a cross-dissolve needs the other clip's picture.
pub struct MultiSource {
    segments: Vec<VideoSegment>,
    target_size: (u32, u32),
    camera: Option<VideoSource>,
    camera2: Option<VideoSource>,
    camera_path: Option<std::path::PathBuf>,
    camera_offset_ms: f64,
}

impl MultiSource {
    pub fn open(package: &crate::Package, project: &screencraft_core::project::Project) -> Result<Self, String> {
        let resolved = project.media.resolved_segments();
        let mut segments: Vec<VideoSegment> = Vec::with_capacity(resolved.len());
        for (i, seg) in resolved.into_iter().enumerate() {
            let path = package.dir.join(&seg.screen);
            let primary = if i == 0 { Some(VideoSource::open(&path)?) } else { None };
            segments.push(VideoSegment {
                path,
                offset_ms: seg.offset_ms,
                duration_ms: seg.duration_ms,
                primary,
                secondary: None,
            });
        }
        let (w0, h0) = segments
            .first()
            .and_then(|s| s.primary.as_ref())
            .map(|v| (v.width, v.height))
            .unwrap_or((project.media.width, project.media.height));
        let target_size = (
            (if project.media.width > 0 { project.media.width } else { w0 }) & !1,
            (if project.media.height > 0 { project.media.height } else { h0 }) & !1,
        );
        let camera_path = project.media.camera.as_ref().map(|c| package.dir.join(c)).filter(|p| p.exists());
        // A camera file that cannot be decoded (an unsupported codec) just leaves the camera out.
        let camera = camera_path.as_ref().and_then(|p| match VideoSource::open(p) {
            Ok(v) => Some(v),
            Err(e) => {
                eprintln!("[screencraft] camera not shown: {e}");
                None
            }
        });
        Ok(Self {
            segments,
            target_size,
            camera,
            camera2: None,
            camera_path,
            camera_offset_ms: project.media.camera_offset_ms,
        })
    }

    pub fn has_camera(&self) -> bool {
        self.camera.is_some()
    }

    fn find_segment_index(&self, t_src: f64) -> usize {
        if self.segments.len() <= 1 {
            return 0;
        }
        for (i, seg) in self.segments.iter().enumerate() {
            if t_src < seg.offset_ms + seg.duration_ms || i + 1 == self.segments.len() {
                return i;
            }
        }
        self.segments.len().saturating_sub(1)
    }

    fn fetch_screen(&mut self, t_src: f64, secondary: bool) -> Result<Arc<I420>, String> {
        let idx = self.find_segment_index(t_src);
        let seg = &mut self.segments[idx];
        let local_t = (t_src - seg.offset_ms).max(0.0);
        let slot = if secondary { &mut seg.secondary } else { &mut seg.primary };
        if slot.is_none() {
            *slot = Some(VideoSource::open(&seg.path)?);
        }
        let pic = slot.as_mut().unwrap().frame_at(local_t)?;
        let (tw, th) = self.target_size;
        if tw > 0 && th > 0 && (pic.width, pic.height) != (tw, th) {
            Ok(Arc::new(crate::append::fit(&pic, tw, th)))
        } else {
            Ok(pic)
        }
    }

    /// Source-frame indexes of the screen and camera for a source time (for change detection).
    pub fn indexes(&self, t_src: f64) -> (usize, Option<usize>) {
        let idx = self.find_segment_index(t_src);
        let seg = &self.segments[idx];
        let local_idx = seg
            .primary
            .as_ref()
            .map(|v| v.index_at((t_src - seg.offset_ms).max(0.0)))
            .unwrap_or_else(|| ((t_src - seg.offset_ms).max(0.0) * 0.06) as usize);
        let global_idx = (idx << 24) | local_idx;
        (global_idx, self.camera.as_ref().map(|c| c.index_at(t_src + self.camera_offset_ms)))
    }

    pub fn fetch(&mut self, t_src: f64, other_t: Option<f64>) -> Result<Frames, String> {
        let screen = self.fetch_screen(t_src, false)?;
        let camera = match self.camera.as_mut() {
            Some(c) => c.frame_at(t_src + self.camera_offset_ms).ok(),
            None => None,
        };
        let (mut screen2, mut camera2) = (None, None);
        if let Some(t2) = other_t {
            screen2 = self.fetch_screen(t2, true).ok();
            if self.camera.is_some() {
                if self.camera2.is_none() {
                    self.camera2 = self.camera_path.as_ref().and_then(|p| VideoSource::open(p).ok());
                }
                camera2 = self.camera2.as_mut().and_then(|s| s.frame_at(t2 + self.camera_offset_ms).ok());
            }
        }
        Ok(Frames { screen, screen2, camera, camera2 })
    }
}

struct AudioSeg {
    path: std::path::PathBuf,
    track: Option<usize>,
    offset_ms: f64,
    duration_ms: f64,
    src: Option<AudioSource>,
}

/// Multi-segment 48 kHz stereo Opus audio source spanning `project.media.segments`.
pub struct SegmentedAudioSource {
    segments: Vec<AudioSeg>,
    pub duration_ms: f64,
}

impl SegmentedAudioSource {
    fn open_kind(package: &crate::Package, project: &screencraft_core::project::Project, mic: bool) -> Option<Self> {
        let resolved = project.media.resolved_segments();
        let mut any = false;
        let mut segments = Vec::with_capacity(resolved.len());
        let mut max_end = project.media.duration_ms;
        for seg in resolved {
            let track = if mic { seg.mic_track } else { seg.system_track }.map(|t| t as usize);
            if track.is_some() {
                any = true;
            }
            max_end = max_end.max(seg.offset_ms + seg.duration_ms);
            segments.push(AudioSeg {
                path: package.dir.join(&seg.screen),
                track,
                offset_ms: seg.offset_ms,
                duration_ms: seg.duration_ms,
                src: None,
            });
        }
        if !any {
            return None;
        }
        Some(Self { segments, duration_ms: max_end })
    }

    pub fn open_system(package: &crate::Package, project: &screencraft_core::project::Project) -> Option<Self> {
        Self::open_kind(package, project, false)
    }

    pub fn open_mic(package: &crate::Package, project: &screencraft_core::project::Project) -> Option<Self> {
        Self::open_kind(package, project, true)
    }

    pub fn read(&mut self, start_ms: f64, len_frames: usize) -> Result<Vec<f32>, String> {
        let mut out = vec![0f32; len_frames * 2];
        let req_end_ms = start_ms + (len_frames as f64 / 48.0);
        for seg in &mut self.segments {
            let Some(track) = seg.track else { continue };
            let seg_end = seg.offset_ms + seg.duration_ms;
            if seg_end <= start_ms || seg.offset_ms >= req_end_ms {
                continue;
            }
            if seg.src.is_none() {
                let file = Arc::new(Mp4File::open(&seg.path).map_err(|e| e.to_string())?);
                seg.src = AudioSource::open(file, track).ok();
            }
            let Some(src) = seg.src.as_mut() else { continue };
            let ov_start = start_ms.max(seg.offset_ms);
            let ov_end = req_end_ms.min(seg_end);
            let dst_off = ((ov_start - start_ms) * 48.0).round() as usize;
            let n_frames = ((ov_end - ov_start) * 48.0).round() as usize;
            let count = n_frames.min(len_frames.saturating_sub(dst_off));
            if count == 0 {
                continue;
            }
            if let Ok(pcm) = src.read((ov_start - seg.offset_ms).max(0.0), count) {
                let n = (count * 2).min(pcm.len()).min(out.len().saturating_sub(dst_off * 2));
                out[dst_off * 2..dst_off * 2 + n].copy_from_slice(&pcm[..n]);
            }
        }
        Ok(out)
    }
}

impl crate::audio_mix::PcmSource for SegmentedAudioSource {
    fn read(&mut self, start_ms: f64, frames: usize) -> Vec<f32> {
        SegmentedAudioSource::read(self, start_ms, frames).unwrap_or_else(|_| vec![0.0; frames * 2])
    }
}
