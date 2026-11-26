//! Add more footage to a project: another `.screencraft` recording, or a plain MP4.
//!
//! Instead of decoding and re-encoding the entire project video, each added clip is copied into the
//! package directory as `clip-N.mp4` and registered in `project.media.segments` on the unified
//! virtual source timeline `[0, duration_ms]`. Decoding and scaling happen on demand during preview
//! and final export, making append near-instant.

use crate::package::Package;
use screencraft_core::input::InputLog;
use screencraft_core::project::{MediaSegment, Project, Rect, Slice};
use screencraft_encode::mp4read::Mp4File;
use screencraft_encode::I420;
use std::path::{Path, PathBuf};

/// One piece of footage to join.
struct ExtraPart {
    segments: Vec<(PathBuf, MediaSegment)>,
    /// Cursor / click log and the captured area, for recordings.
    input: Option<InputLog>,
    bounds: Option<Rect>,
}

fn open_extra(path: &Path) -> Result<ExtraPart, String> {
    // A `.screencraft` folder, or any file inside one, is a recording.
    let pkg_dir = if path.is_dir() {
        Some(path.to_path_buf())
    } else {
        path.parent().filter(|p| p.join("project.json").exists()).map(Path::to_path_buf)
    };
    if let Some(dir) = pkg_dir.filter(|d| d.join("project.json").exists()) {
        let pkg = Package::open(dir);
        let p = pkg.load_project()?;
        if p.media.mic.is_some() {
            return Err("this recording keeps its microphone in a separate file, which can't be joined yet".into());
        }
        let mut segments = Vec::new();
        for seg in p.media.resolved_segments() {
            let seg_path = pkg.dir.join(&seg.screen);
            let file = Mp4File::open(&seg_path).map_err(|e| e.to_string())?;
            let v = file.video().ok_or("a segment has no video")?;
            if &v.codec != b"avc1" {
                return Err(format!("only H.264 videos can be added ('{}' is not supported yet)", v.codec_str()));
            }
            let dur = if seg.duration_ms > 0.0 {
                seg.duration_ms
            } else if v.duration_us > 0 {
                v.duration_us as f64 / 1000.0
            } else {
                file.duration_us as f64 / 1000.0
            };
            let mut s = seg;
            s.duration_ms = dur;
            if s.width == 0 || s.height == 0 {
                s.width = v.width & !1;
                s.height = v.height & !1;
            }
            segments.push((seg_path, s));
        }
        let input = pkg.load_input(&p);
        return Ok(ExtraPart {
            segments,
            input: Some(input),
            bounds: Some(p.media.bounds),
        });
    }

    let file = Mp4File::open(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let v = file.video().ok_or("the file has no video")?;
    if &v.codec != b"avc1" {
        return Err(format!("only H.264 videos can be added ('{}' is not supported yet)", v.codec_str()));
    }
    let duration_ms = if v.duration_us > 0 { v.duration_us as f64 / 1000.0 } else { file.duration_us as f64 / 1000.0 };
    // A plain video's first Opus track counts as its sound (other codecs aren't decoded).
    let system_track = file.audio_tracks().enumerate().find(|(_, t)| &t.codec == b"Opus").map(|(i, _)| i as u32);
    let seg = MediaSegment {
        screen: String::new(),
        offset_ms: 0.0,
        duration_ms,
        width: v.width & !1,
        height: v.height & !1,
        system_track,
        mic_track: None,
    };
    Ok(ExtraPart {
        segments: vec![(path.to_path_buf(), seg)],
        input: None,
        bounds: None,
    })
}

/// Box-filter `f` into a `w`×`h` picture, keeping its shape (black bars around it).
pub(crate) fn fit(f: &I420, w: u32, h: u32) -> I420 {
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

/// What `append` did: the old screen video (empty when kept via multi-segment mode), added duration, and new slice ID.
pub struct Appended {
    pub old_screen: PathBuf,
    pub added_ms: f64,
    pub slice_id: String,
}

/// Join the footage at `extra` (a `.screencraft` folder or an MP4) onto the project in `target`,
/// inserting its clip at `insert_at` (or at the end when `None`). `progress` gets 0..1 and returns
/// false to cancel.
pub fn append_at(target: &Path, extra: &Path, insert_at: Option<usize>, mut progress: impl FnMut(f32) -> bool) -> Result<Appended, String> {
    let pkg = Package::open(target);
    let mut project: Project = pkg.load_project()?;
    if project.media.mic.is_some() {
        return Err("this project keeps its microphone in a separate file, which can't be joined yet".into());
    }
    if extra.canonicalize().ok() == pkg.dir.canonicalize().ok() {
        return Err("that is this project".into());
    }
    if !progress(0.1) {
        return Err("cancelled".into());
    }

    let extra_part = open_extra(extra)?;
    if project.media.segments.is_empty() {
        project.media.segments = project.media.resolved_segments();
    }

    let base_ms = project.media.duration_ms;
    let mut cur_ms = base_ms;
    let mut copied_files: Vec<PathBuf> = Vec::new();
    let cleanup = |files: &[PathBuf], e: String| {
        for f in files {
            let _ = std::fs::remove_file(f);
        }
        e
    };

    let n_segs = extra_part.segments.len().max(1);
    for (idx, (src_path, mut seg)) in extra_part.segments.into_iter().enumerate() {
        let clip_name = fresh_name(&pkg.dir, "clip", "mp4");
        let dst_path = pkg.dir.join(&clip_name);
        std::fs::copy(&src_path, &dst_path).map_err(|e| cleanup(&copied_files, format!("cannot copy {}: {e}", src_path.display())))?;
        copied_files.push(dst_path);
        seg.screen = clip_name;
        seg.offset_ms = cur_ms;
        cur_ms += seg.duration_ms;
        project.media.segments.push(seg);
        if !progress(0.15 + 0.65 * ((idx + 1) as f32 / n_segs as f32)) {
            return Err(cleanup(&copied_files, "cancelled".into()));
        }
    }
    let added_ms = cur_ms - base_ms;

    // Cursor log & auto-zoom for the newly added recording (skipped for webcam-only projects).
    if !project.media.camera_only {
        if let (Some(extra_log), Some(from)) = (&extra_part.input, extra_part.bounds) {
            let mut log = pkg.load_input(&project);
            let moved = shift_input(extra_log, from, project.media.bounds, base_ms);
            log.moves.extend(moved.moves);
            log.clicks.extend(moved.clicks);
            log.keys.extend(moved.keys);
            log.carets.extend(moved.carets);
            let input_name = fresh_name(&pkg.dir, "input", "json");
            let data = serde_json::to_vec(&log).map_err(|e| cleanup(&copied_files, e.to_string()))?;
            std::fs::write(pkg.dir.join(&input_name), data).map_err(|e| cleanup(&copied_files, e.to_string()))?;
            project.media.input = input_name;

            // Generate auto-zooms for the newly added recording segment and shift them to [base_ms, cur_ms].
            let local_log = shift_input(extra_log, from, project.media.bounds, 0.0);
            let opts = screencraft_core::zoom::AutoZoomOptions::for_ease(
                project.config.animations.default_zoom,
                project.config.animations.ease_in_ms,
            );
            let mut next_z = 0usize;
            for mut z in screencraft_core::zoom::generate(&local_log, added_ms, &opts) {
                z.source_start_ms += base_ms;
                z.source_end_ms += base_ms;
                while project.scene.zooms.iter().any(|existing| existing.id == format!("zoom-add-{next_z}")) {
                    next_z += 1;
                }
                z.id = format!("zoom-add-{next_z}");
                next_z += 1;
                project.scene.zooms.push(z);
            }
        }
    }

    let has_sys = project.media.segments.iter().any(|s| s.system_track.is_some());
    let has_mic = project.media.segments.iter().any(|s| s.mic_track.is_some());
    project.media.duration_ms = cur_ms;
    project.media.has_system_audio = has_sys;
    project.media.has_microphone = has_mic;

    let n = (0..).find(|i| !project.scene.slices.iter().any(|s| s.id == format!("slice-add-{i}"))).unwrap();
    let slice_id = format!("slice-add-{n}");
    let new_slice = Slice {
        id: slice_id.clone(),
        source_start_ms: base_ms,
        source_end_ms: cur_ms,
        ..Default::default()
    };
    let idx = insert_at.map(|i| i.min(project.scene.slices.len())).unwrap_or(project.scene.slices.len());
    project.scene.slices.insert(idx, new_slice);

    pkg.save_project(&project).map_err(|e| cleanup(&copied_files, e))?;
    progress(1.0);
    Ok(Appended { old_screen: PathBuf::new(), added_ms, slice_id })
}

/// Join the footage at `extra` (a `.screencraft` folder or an MP4) onto the end of the project in
/// `target`. `progress` gets 0..1 and returns false to cancel.
pub fn append(target: &Path, extra: &Path, progress: impl FnMut(f32) -> bool) -> Result<Appended, String> {
    append_at(target, extra, None, progress)
}
