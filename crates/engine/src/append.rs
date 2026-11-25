//! Add more footage to the end of a project: another `.screencraft` recording, or a plain MP4.
//!
//! A project plays one screen video, so the project's video and the new one are decoded and
//! encoded again, one after the other, into a new `screen-N.mp4` at the project's size (the new
//! part is fitted inside with black bars when its shape differs). Audio tracks are carried over
//! (silence where a part has none), the cursor log of a recording is moved after the first part,
//! and a new clip for the added part goes at the end of the timeline. Every edit already made
//! stays where it was. The old video file is left for the caller to remove.

use crate::package::Package;
use screencraft_core::input::InputLog;
use screencraft_core::project::{Project, Rect, Slice};
use screencraft_encode::mp4read::{parse_avcc, Mp4File, Track};
use screencraft_encode::opus::OpusDecoder;
use screencraft_encode::{AudioConfig, Container, Effort, ExportSettings, FileSink, HwPreference, ThreadedExporter, VideoCodec, VideoConfig, I420};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// One piece of footage to join.
struct Part {
    file: Arc<Mp4File>,
    duration_ms: f64,
    system: Option<Track>,
    mic: Option<Track>,
    /// Cursor / click log and the captured area, for recordings.
    input: Option<InputLog>,
    bounds: Option<Rect>,
}

fn audio_track(file: &Mp4File, index: Option<u32>) -> Option<Track> {
    let i = index? as usize;
    file.audio_tracks().nth(i).filter(|t| &t.codec == b"Opus").cloned()
}

fn open_part(path: &Path) -> Result<Part, String> {
    // A `.screencraft` folder, or any file inside one, is a recording.
    let pkg_dir = if path.is_dir() { Some(path.to_path_buf()) } else { path.parent().filter(|p| p.join("project.json").exists()).map(Path::to_path_buf) };
    if let Some(dir) = pkg_dir.filter(|d| d.join("project.json").exists()) {
        let pkg = Package::open(dir);
        let p = pkg.load_project()?;
        if p.media.mic.is_some() {
            return Err("this recording keeps its microphone in a separate file, which can't be joined yet".into());
        }
        let file = Arc::new(Mp4File::open(&pkg.screen_path(&p)).map_err(|e| e.to_string())?);
        let input = pkg.load_input(&p);
        return Ok(Part {
            system: audio_track(&file, p.media.system_track.or(if p.media.has_system_audio && p.media.mic_track.is_none() { Some(0) } else { None })),
            mic: audio_track(&file, p.media.mic_track),
            duration_ms: p.media.duration_ms,
            input: Some(input),
            bounds: Some(p.media.bounds),
            file,
        });
    }
    let file = Arc::new(Mp4File::open(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?);
    let v = file.video().ok_or("the file has no video")?;
    let duration_ms = if v.duration_us > 0 { v.duration_us as f64 / 1000.0 } else { file.duration_us as f64 / 1000.0 };
    // A plain video's first Opus track counts as its sound (other codecs aren't decoded).
    let system = file.audio_tracks().find(|t| &t.codec == b"Opus").cloned();
    Ok(Part { file, duration_ms, system, mic: None, input: None, bounds: None })
}

/// Box-filter `f` into a `w`×`h` picture, keeping its shape (black bars around it).
fn fit(f: &I420, w: u32, h: u32) -> I420 {
    if (f.width, f.height) == (w, h) {
        return f.clone();
    }
    let k = (w as f64 / f.width as f64).min(h as f64 / f.height as f64);
    let (iw, ih) = ((((f.width as f64 * k) as u32) & !1).max(2), (((f.height as f64 * k) as u32) & !1).max(2));
    let inner = crate::compress::scale(f, iw, ih);
    let mut out = I420::new(w, h);
    out.y.iter_mut().for_each(|p| *p = 16);
    out.u.iter_mut().for_each(|p| *p = 128);
    out.v.iter_mut().for_each(|p| *p = 128);
    let (ox, oy) = (((w - iw) / 2) & !1, ((h - ih) / 2) & !1);
    for y in 0..ih as usize {
        let d = (oy as usize + y) * w as usize + ox as usize;
        out.y[d..d + iw as usize].copy_from_slice(&inner.y[y * iw as usize..(y + 1) * iw as usize]);
    }
    let (cw, cow, cox, coy) = (iw as usize / 2, w as usize / 2, ox as usize / 2, oy as usize / 2);
    for y in 0..ih as usize / 2 {
        let d = (coy + y) * cow + cox;
        out.u[d..d + cw].copy_from_slice(&inner.u[y * cw..(y + 1) * cw]);
        out.v[d..d + cw].copy_from_slice(&inner.v[y * cw..(y + 1) * cw]);
    }
    out
}

/// Decoded stereo 48 kHz audio of `track`, exactly `frames` long (padded with silence or trimmed).
fn decode_audio(file: &Mp4File, track: Option<&Track>, frames: usize) -> Result<Vec<f32>, String> {
    let mut out = Vec::with_capacity(frames * 2);
    if let Some(a) = track {
        let ch = a.channels.clamp(1, 2) as usize;
        let mut od = OpusDecoder::new(ch as u8).map_err(|e| e.to_string())?;
        let pre_skip = if a.config.len() >= 4 { u16::from_be_bytes([a.config[2], a.config[3]]) as usize } else { 312 };
        let mut skip = pre_skip * 2;
        for s in &a.samples {
            let raw = file.read_sample(s).map_err(|e| e.to_string())?;
            let pcm = od.decode(&raw).map_err(|e| e.to_string())?;
            let stereo: Vec<f32> = if ch == 1 { pcm.iter().flat_map(|x| [*x, *x]).collect() } else { pcm };
            let n = skip.min(stereo.len());
            skip -= n;
            out.extend_from_slice(&stereo[n..]);
            if out.len() >= frames * 2 {
                break;
            }
        }
    }
    out.resize(frames * 2, 0.0);
    Ok(out)
}

/// Move `log` (recorded over `from`) to start at `offset_ms`, mapping its positions onto `to`.
fn shift_input(log: &InputLog, from: Rect, to: Rect, offset_ms: f64) -> InputLog {
    let map = |x: f64, y: f64| {
        let nx = if from.width > 0.0 { (x - from.x) / from.width } else { 0.5 };
        let ny = if from.height > 0.0 { (y - from.y) / from.height } else { 0.5 };
        (to.x + nx * to.width, to.y + ny * to.height)
    };
    let sx = if from.width > 0.0 { to.width / from.width } else { 1.0 };
    let sy = if from.height > 0.0 { to.height / from.height } else { 1.0 };
    let mut l = log.clone();
    for m in &mut l.moves {
        (m.x, m.y) = map(m.x, m.y);
        m.t += offset_ms;
    }
    for c in &mut l.clicks {
        (c.x, c.y) = map(c.x, c.y);
        c.t += offset_ms;
    }
    for k in &mut l.keys {
        k.t += offset_ms;
    }
    for c in &mut l.carets {
        (c.x, c.y) = map(c.x, c.y);
        c.w *= sx;
        c.h *= sy;
        c.t += offset_ms;
    }
    l
}

/// A file name in `dir` not taken yet: `stem-2.ext`, `stem-3.ext` …
fn fresh_name(dir: &Path, stem: &str, ext: &str) -> String {
    (2..).map(|n| format!("{stem}-{n}.{ext}")).find(|n| !dir.join(n).exists()).unwrap()
}

/// What `append` did: the old screen video, now unused, for the caller to delete.
pub struct Appended {
    pub old_screen: PathBuf,
    pub added_ms: f64,
}

/// Join the footage at `extra` (a `.screencraft` folder or an MP4) onto the end of the project in
/// `target`. `progress` gets 0..1 and returns false to cancel.
pub fn append(target: &Path, extra: &Path, mut progress: impl FnMut(f32) -> bool) -> Result<Appended, String> {
    let pkg = Package::open(target);
    let mut project: Project = pkg.load_project()?;
    if project.media.camera_only {
        return Err("adding footage to a camera-only video isn't supported yet".into());
    }
    if project.media.mic.is_some() {
        return Err("this project keeps its microphone in a separate file, which can't be joined yet".into());
    }
    let base = open_part(&pkg.dir)?;
    let extra_part = open_part(extra)?;
    if extra.canonicalize().ok() == pkg.dir.canonicalize().ok() {
        return Err("that is this project".into());
    }
    let parts = [base, extra_part];

    let v0 = parts[0].file.video().ok_or("the project has no video")?;
    let (w, h) = (v0.width & !1, v0.height & !1);
    let fps = {
        let s = &v0.samples;
        if s.len() > 1 { ((s.len() as f64 - 1.0) * 1e6 / (s.last().unwrap().pts_us - s[0].pts_us).max(1) as f64).round().clamp(10.0, 120.0) } else { 30.0 }
    };
    let has_sys = parts.iter().any(|p| p.system.is_some());
    let has_mic = parts.iter().any(|p| p.mic.is_some());
    let mut audio_cfg = Vec::new();
    if has_sys {
        audio_cfg.push(AudioConfig::default());
    }
    if has_mic {
        audio_cfg.push(AudioConfig::default());
    }
    let mic_index = if has_sys { 1 } else { 0 };

    let mut video = VideoConfig::new(VideoCodec::H264, w, h, fps as u32);
    video.bitrate_kbps = screencraft_encode::default_bitrate_kbps(w, h, fps as u32);
    video.effort = Effort::Balanced;
    video.hardware = HwPreference::Auto;

    let screen_name = fresh_name(&pkg.dir, "screen", "mp4");
    let out_path = pkg.dir.join(&screen_name);
    let sink = FileSink::create(&out_path).map_err(|e| format!("cannot create {}: {e}", out_path.display()))?;
    let exporter = ThreadedExporter::spawn(ExportSettings { container: Container::Mp4, video, audio: audio_cfg }, Box::new(sink)).map_err(|e| e.to_string())?;
    let fail = |e: String| {
        let _ = std::fs::remove_file(&out_path);
        e
    };

    let total_samples: usize = parts.iter().filter_map(|p| p.file.video()).map(|v| v.samples.len()).sum::<usize>().max(1);
    let mut done = 0usize;
    let mut offset_us = 0i64;
    let mut last_pts = -1i64;
    for part in &parts {
        let v = part.file.video().ok_or_else(|| fail("a part has no video".into()))?;
        if &v.codec != b"avc1" {
            return Err(fail(format!("only H.264 videos can be added ('{}' is not supported yet)", v.codec_str())));
        }
        let (plen, params) = parse_avcc(&v.config).map_err(|e| fail(e.to_string()))?;
        let mut dec = screencraft_encode::h264::H264Decoder::new(&params, plen).map_err(|e| fail(e.to_string()))?;
        let t0 = v.samples.first().map(|s| s.pts_us).unwrap_or(0);
        let part_us = (part.duration_ms * 1000.0) as i64;
        for s in &v.samples {
            let raw = part.file.read_sample(s).map_err(|e| fail(e.to_string()))?;
            if let Some(pic) = dec.decode(&raw).map_err(|e| fail(e.to_string()))? {
                let pts = (offset_us + (s.pts_us - t0).clamp(0, part_us.max(1) - 1)).max(last_pts + 1);
                last_pts = pts;
                exporter.push_video(Arc::new(fit(&pic, w, h)), pts).map_err(|e| fail(e.to_string()))?;
            }
            done += 1;
            if done % 8 == 0 && !progress(done as f32 / total_samples as f32 * 0.85) {
                drop(exporter);
                let _ = std::fs::remove_file(&out_path);
                return Err("cancelled".into());
            }
        }
        offset_us += part_us;
    }
    // audio, part after part, each exactly as long as its video
    for part in &parts {
        let frames = (part.duration_ms * 48.0).round() as usize;
        if has_sys {
            let pcm = decode_audio(&part.file, part.system.as_ref(), frames).map_err(fail)?;
            exporter.push_audio(0, pcm).map_err(|e| fail(e.to_string()))?;
        }
        if has_mic {
            let pcm = decode_audio(&part.file, part.mic.as_ref(), frames).map_err(fail)?;
            exporter.push_audio(mic_index, pcm).map_err(|e| fail(e.to_string()))?;
        }
    }
    progress(0.92);
    exporter.finish().map_err(|e| fail(e.to_string()))?;

    // cursor log: the project's own, then the added part's, moved after it
    let base_ms = parts[0].duration_ms;
    let added_ms = parts[1].duration_ms;
    let mut log = parts[0].input.clone().unwrap_or_default();
    if let (Some(extra_log), Some(from)) = (&parts[1].input, parts[1].bounds) {
        let moved = shift_input(extra_log, from, project.media.bounds, base_ms);
        log.moves.extend(moved.moves);
        log.clicks.extend(moved.clicks);
        log.keys.extend(moved.keys);
        log.carets.extend(moved.carets);
    }
    let input_name = fresh_name(&pkg.dir, "input", "json");
    std::fs::write(pkg.dir.join(&input_name), serde_json::to_vec(&log).map_err(|e| fail(e.to_string()))?).map_err(|e| fail(e.to_string()))?;

    // the project: longer media, the new audio layout, and one more clip at the end
    let old_screen = pkg.screen_path(&project);
    let m = &mut project.media;
    m.screen = screen_name;
    m.input = input_name;
    m.duration_ms = base_ms + added_ms;
    m.width = w;
    m.height = h;
    m.has_system_audio = has_sys;
    m.has_microphone = has_mic;
    m.system_track = has_sys.then_some(0);
    m.mic_track = has_mic.then_some(mic_index as u32);
    let n = (0..).find(|i| !project.scene.slices.iter().any(|s| s.id == format!("slice-add-{i}"))).unwrap();
    project.scene.slices.push(Slice { id: format!("slice-add-{n}"), source_start_ms: base_ms, source_end_ms: base_ms + added_ms, ..Default::default() });
    pkg.save_project(&project).map_err(fail)?;
    progress(1.0);
    Ok(Appended { old_screen, added_ms })
}
