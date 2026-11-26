//! Headless MCP (Model Context Protocol) server exposing ScreenCraft project inspection,
//! screen recording, timeline editing, captions, frame rendering, and video export over stdio JSON-RPC.

use base64::Engine;
use screencraft_capture::camera::{CamPreset, CameraHub, CameraStream};
use screencraft_capture::{list_sources, local_time, Target};
use screencraft_core::jumpcut::{apply_jump_cuts, clear_jump_cuts, cut_total_ms, find_silences, heal, JumpCutSettings};
use screencraft_core::presets::{apply_subtitle_preset, builtin_presets, deep_merge, find_and_apply_preset};
use screencraft_core::project::{Annotation, Point, Project, Rect, Slice, ZoomKind, ZoomRegion};
use screencraft_core::timeline::{effective_duration_ms, output_duration_ms};
use screencraft_core::transcript::{cut_ranges, extract_words, restore_ranges};
use screencraft_core::zoom::{generate as generate_auto_zoom, AutoZoomOptions};
use screencraft_encode::{available_video_codecs, default_bitrate_kbps, Container, Effort, HwPreference, VideoCodec};
use screencraft_engine::jumpcuts::{audio_levels, FRAME_MS};
use screencraft_engine::package::list_packages;
use screencraft_engine::whisper::{transcribe_sync, Request as WhisperRequest};
use screencraft_engine::{export_gif, export_project, render_frame_at, CameraSpec, ExportOptions, Package, RecordOptions, Recording};
use screencraft_render::output_size;
use serde_json::{json, Value};
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

struct ActiveRecording {
    recording: Recording,
    camera_stream: Option<CameraStream>,
}

pub struct McpServer {
    recording: Option<ActiveRecording>,
    recordings_dir: PathBuf,
    id_counter: u64,
}

impl Default for McpServer {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for McpServer {
    fn drop(&mut self) {
        let _ = self.recording.take();
    }
}

impl McpServer {
    pub fn new() -> Self {
        Self {
            recording: None,
            recordings_dir: default_recordings_dir(),
            id_counter: 0,
        }
    }

    pub fn with_recordings_dir(recordings_dir: PathBuf) -> Self {
        Self {
            recording: None,
            recordings_dir,
            id_counter: 0,
        }
    }

    fn next_id(&mut self, prefix: &str) -> String {
        self.id_counter += 1;
        let t = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_micros())
            .unwrap_or(0);
        format!("{prefix}-{t:x}-{}", self.id_counter)
    }

    /// Process a single JSON-RPC 2.0 line; returns `None` for notifications.
    pub fn handle_message(&mut self, raw: &str) -> Option<Value> {
        let msg: Value = match serde_json::from_str(raw) {
            Ok(v) => v,
            Err(e) => return Some(rpc_error(Value::Null, -32700, format!("Parse error: {e}"))),
        };
        let id = msg.get("id").cloned();
        let Some(method) = msg.get("method").and_then(Value::as_str) else {
            return id.map(|i| rpc_error(i, -32600, "Invalid Request: missing method".into()));
        };
        let params = msg.get("params").cloned().unwrap_or_else(|| json!({}));

        // Notifications carry no `id` and expect no reply.
        let Some(id) = id else {
            return None;
        };

        let result = match method {
            "initialize" => {
                let version = params
                    .get("protocolVersion")
                    .and_then(Value::as_str)
                    .unwrap_or("2024-11-05");
                Ok(json!({
                    "protocolVersion": version,
                    "capabilities": {
                        "tools": {}
                    },
                    "serverInfo": {
                        "name": "screencraft-mcp",
                        "version": env!("CARGO_PKG_VERSION")
                    }
                }))
            }
            "ping" => Ok(json!({})),
            "tools/list" => Ok(json!({ "tools": tool_definitions() })),
            "tools/call" => {
                let name = params.get("name").and_then(Value::as_str).unwrap_or("");
                let args = params.get("arguments").cloned().unwrap_or_else(|| json!({}));
                match self.call_tool(name, &args) {
                    Ok(v) => Ok(v),
                    Err(err) => Ok(tool_error(err)),
                }
            }
            _ => return Some(rpc_error(id, -32601, format!("Method not found: {method}"))),
        };

        Some(match result {
            Ok(res) => json!({ "jsonrpc": "2.0", "id": id, "result": res }),
            Err(err) => rpc_error(id, -32602, err),
        })
    }

    fn call_tool(&mut self, name: &str, args: &Value) -> Result<Value, String> {
        match name {
            "list_projects" => self.tool_list_projects(args),
            "get_project_summary" => self.tool_get_project_summary(args),
            "list_capture_sources" => self.tool_list_capture_sources(),
            "start_recording" => self.tool_start_recording(args),
            "stop_recording" => self.tool_stop_recording(args),
            "trim_or_cut" => self.tool_trim_or_cut(args),
            "set_clip_speed" => self.tool_set_clip_speed(args),
            "add_zoom" => self.tool_add_zoom(args),
            "add_annotation" => self.tool_add_annotation(args),
            "apply_preset" => self.tool_apply_preset(args),
            "generate_captions" => self.tool_generate_captions(args),
            "render_frame" => self.tool_render_frame(args),
            "export_video" => self.tool_export_video(args),
            _ => Err(format!("Unknown tool: {name}")),
        }
    }

    fn tool_list_projects(&self, args: &Value) -> Result<Value, String> {
        let dir = str_arg(args, "directory")
            .map(PathBuf::from)
            .unwrap_or_else(|| self.recordings_dir.clone());
        let pkgs = list_packages(&dir);
        let items: Vec<Value> = pkgs
            .into_iter()
            .filter_map(|(pkg, modified)| {
                let p = pkg.load_project().ok()?;
                let eff_ms = if p.scene.slices.is_empty() {
                    p.media.duration_ms
                } else {
                    effective_duration_ms(&p.scene.slices)
                };
                Some(json!({
                    "path": pkg.dir.to_string_lossy(),
                    "name": p.name,
                    "modified_unix_s": modified,
                    "duration_ms": p.media.duration_ms,
                    "effective_duration_ms": eff_ms,
                    "width": p.media.width,
                    "height": p.media.height,
                    "has_camera": p.media.camera.is_some() || p.media.camera_only,
                    "camera_only": p.media.camera_only,
                    "has_microphone": p.media.has_microphone || p.media.mic.is_some(),
                    "has_system_audio": p.media.has_system_audio,
                    "zooms": p.scene.zooms.len(),
                    "annotations": p.scene.annotations.len(),
                    "subtitles": p.scene.subtitles.len(),
                }))
            })
            .collect();
        Ok(tool_json(json!({ "directory": dir.to_string_lossy(), "projects": items })))
    }

    fn tool_get_project_summary(&self, args: &Value) -> Result<Value, String> {
        let (pkg, project) = resolve_package(req_str(args, "project_path")?)?;
        let input = pkg.load_input(&project);
        let words = extract_words(&project.scene.subtitles, &project.scene.slices);
        let transcript_text = words
            .iter()
            .filter(|w| !w.cut)
            .map(|w| w.text.as_str())
            .collect::<Vec<_>>()
            .join(" ");
        let transcript_words: Vec<Value> = words
            .iter()
            .map(|w| {
                json!({
                    "text": w.text,
                    "start_ms": w.start_ms,
                    "end_ms": w.end_ms,
                    "cut": w.cut,
                    "caption_id": w.caption_id,
                })
            })
            .collect();

        let clicks: Vec<Value> = input
            .clicks
            .iter()
            .filter(|c| c.down)
            .take(500)
            .map(|c| {
                let (nx, ny) = input.normalize(c.x, c.y);
                json!({ "t_ms": c.t, "x": nx, "y": ny, "button": c.button })
            })
            .collect();
        let shortcuts: Vec<Value> = input
            .keys
            .iter()
            .filter(|k| k.down && !k.label.is_empty())
            .take(500)
            .map(|k| json!({ "t_ms": k.t, "label": k.label, "mods": k.mods }))
            .collect();

        let (out_w, out_h) = output_size(&project, project.config.output.resolution);
        Ok(tool_json(json!({
            "project_path": pkg.dir.to_string_lossy(),
            "name": project.name,
            "media": project.media,
            "output": {
                "aspect_ratio": project.config.output.aspect_ratio,
                "resolution": project.config.output.resolution,
                "width": out_w,
                "height": out_h,
                "fps": project.config.output.fps,
            },
            "timeline": {
                "output_duration_ms": output_duration_ms(&project.scene.slices),
                "effective_duration_ms": effective_duration_ms(&project.scene.slices),
                "cut_duration_ms": cut_total_ms(&project.scene.slices),
                "slices": project.scene.slices,
            },
            "zooms": project.scene.zooms,
            "annotations": project.scene.annotations,
            "camera_layouts": project.scene.camera_layouts,
            "subtitles": project.scene.subtitles,
            "transcript": {
                "text": transcript_text,
                "words": transcript_words,
            },
            "input_events": {
                "total_clicks": input.clicks.iter().filter(|c| c.down).count(),
                "total_keys": input.keys.iter().filter(|k| k.down).count(),
                "clicks": clicks,
                "shortcuts": shortcuts,
            }
        })))
    }

    fn tool_list_capture_sources(&self) -> Result<Value, String> {
        let sources = list_sources()?;
        let cameras: Vec<Value> = screencraft_capture::camera::list()
            .into_iter()
            .map(|(id, name)| json!({ "id": id, "name": name }))
            .collect();
        Ok(tool_json(json!({
            "displays": sources.displays,
            "windows": sources.windows,
            "cameras": cameras,
        })))
    }

    fn tool_start_recording(&mut self, args: &Value) -> Result<Value, String> {
        if self.recording.is_some() {
            return Err("A recording is already active. Call stop_recording first.".into());
        }
        let target_kind = str_arg(args, "target").unwrap_or("display");
        let vertical = args.get("vertical").and_then(Value::as_bool).unwrap_or(false);
        let camera_only = target_kind == "camera";
        let target = match target_kind {
            "display" => Target::Display(args.get("display_id").and_then(Value::as_u64).map(|v| v as u32)),
            "window" => {
                let wid = args
                    .get("window_id")
                    .and_then(Value::as_u64)
                    .ok_or("window_id is required when target is 'window'")? as u32;
                Target::Window(wid)
            }
            "area" => {
                let area = args.get("area").ok_or("area object {x, y, width, height} is required when target is 'area'")?;
                let rect = Rect {
                    x: area.get("x").and_then(Value::as_f64).ok_or("area.x is required")?,
                    y: area.get("y").and_then(Value::as_f64).ok_or("area.y is required")?,
                    width: area.get("width").and_then(Value::as_f64).ok_or("area.width is required")?,
                    height: area.get("height").and_then(Value::as_f64).ok_or("area.height is required")?,
                };
                Target::Area {
                    display: args.get("display_id").and_then(Value::as_u64).map(|v| v as u32),
                    rect,
                }
            }
            "camera" => Target::Camera { vertical },
            other => return Err(format!("Unsupported recording target '{other}' (expected display, window, area, or camera)")),
        };

        let fps = args.get("fps").and_then(Value::as_u64).unwrap_or(30).clamp(15, 120) as u32;
        let max_long_side = match args.get("max_long_side").and_then(Value::as_u64) {
            Some(0) => None,
            Some(v) => Some(v as u32),
            None => if camera_only { None } else { Some(2560) },
        };
        let system_audio = if camera_only { false } else { args.get("system_audio").and_then(Value::as_bool).unwrap_or(true) };
        let microphone = args.get("microphone").and_then(Value::as_bool).unwrap_or(true);
        let microphone_id = str_arg(args, "microphone_id").map(String::from);
        let use_camera = camera_only || args.get("camera").and_then(Value::as_bool).unwrap_or(false);
        let camera_id = str_arg(args, "camera_id").map(String::from);

        let (camera_stream, camera_spec) = if use_camera {
            let preset = if camera_only {
                if vertical {
                    CamPreset::TalkingHeadVertical
                } else {
                    CamPreset::TalkingHead
                }
            } else {
                CamPreset::Bubble
            };
            let hub = CameraHub::new();
            let h2 = hub.clone();
            let stream = screencraft_capture::camera::start(camera_id.as_deref(), preset, move |f| h2.publish(f))?;
            (Some(stream), Some(CameraSpec { id: camera_id, hub }))
        } else {
            (None, None)
        };

        let root = str_arg(args, "output_dir")
            .map(PathBuf::from)
            .unwrap_or_else(|| self.recordings_dir.clone());
        std::fs::create_dir_all(&root).map_err(|e| e.to_string())?;
        let pkg = if let Some(name) = str_arg(args, "name") {
            let safe: String = name.chars().map(|c| if "/\\:*?\"<>|".contains(c) { '-' } else { c }).collect();
            let mut dir = root.join(format!("{safe}.screencraft"));
            let mut n = 2;
            while dir.exists() {
                dir = root.join(format!("{safe} {n}.screencraft"));
                n += 1;
            }
            std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
            Package::open(dir)
        } else {
            Package::create_in(&root, &local_stamp()).map_err(|e| e.to_string())?
        };

        let rec = Recording::start(RecordOptions {
            target,
            fps,
            max_long_side,
            system_audio,
            microphone,
            microphone_id,
            output: pkg.clone(),
            camera: camera_spec,
            bitrate_kbps: None,
            hardware: HwPreference::Auto,
        })?;
        let info = rec.info().clone();
        let encoder = rec.encoder().to_string();
        self.recording = Some(ActiveRecording {
            recording: rec,
            camera_stream,
        });
        Ok(tool_json(json!({
            "status": "recording",
            "camera_only": camera_only,
            "project_path": pkg.dir.to_string_lossy(),
            "width": info.width,
            "height": info.height,
            "fps": fps,
            "encoder": encoder,
        })))
    }

    fn tool_stop_recording(&mut self, args: &Value) -> Result<Value, String> {
        let ActiveRecording { recording, camera_stream } = self.recording.take().ok_or("No active recording to stop.")?;
        let fin_res = recording.stop();
        if let Some(cs) = camera_stream {
            cs.stop();
        }
        let mut fin = fin_res?;
        let auto_zoom = args.get("auto_zoom").and_then(Value::as_bool).unwrap_or(true);
        let auto_jump_cuts = args.get("auto_jump_cuts").and_then(Value::as_bool).unwrap_or(false);
        let mut edited = false;
        if auto_zoom && !fin.project.media.camera_only {
            let input = fin.package.load_input(&fin.project);
            let opts = AutoZoomOptions::for_ease(
                fin.project.config.animations.default_zoom,
                fin.project.config.animations.ease_in_ms,
            );
            let mut regions = generate_auto_zoom(&input, fin.project.media.duration_ms, &opts);
            for r in &mut regions {
                r.id = self.next_id("zoom");
            }
            fin.project.scene.zooms = regions;
            edited = true;
        }
        if auto_jump_cuts && (fin.project.media.has_microphone || fin.project.media.has_system_audio || fin.project.media.mic.is_some()) {
            let levels = audio_levels(&fin.package, &fin.project)?;
            let ranges = find_silences(&levels, FRAME_MS, &JumpCutSettings::default());
            clear_jump_cuts(&mut fin.project.scene.slices);
            let mut ids: Vec<String> = (0..128).map(|_| self.next_id("slice")).collect();
            apply_jump_cuts(&mut fin.project.scene.slices, &ranges, || ids.pop().unwrap_or_else(|| "slice-x".into()));
            heal(&mut fin.project.scene.slices);
            edited = true;
        }
        if edited {
            fin.package.save_project(&fin.project)?;
        }
        Ok(tool_json(json!({
            "status": "stopped",
            "camera_only": fin.project.media.camera_only,
            "project_path": fin.package.dir.to_string_lossy(),
            "name": fin.project.name,
            "duration_ms": fin.duration_ms,
            "frames": fin.frames,
            "encoder": fin.encoder,
            "zooms": fin.project.scene.zooms.len(),
            "slices": fin.project.scene.slices.len(),
        })))
    }

    fn tool_trim_or_cut(&mut self, args: &Value) -> Result<Value, String> {
        let (pkg, mut project) = resolve_package(req_str(args, "project_path")?)?;
        let mode = str_arg(args, "mode").unwrap_or("cut");
        let mut ids: Vec<String> = (0..128).map(|_| self.next_id("slice")).collect();
        let mut next_id = || ids.pop().unwrap_or_else(|| "slice-x".into());

        match mode {
            "cut" => {
                let ranges = parse_ranges(args)?;
                cut_ranges(&mut project.scene.slices, &ranges, &mut next_id);
            }
            "restore" => {
                let ranges = parse_ranges(args)?;
                restore_ranges(&mut project.scene.slices, &ranges, &mut next_id);
            }
            "delete" => {
                let ranges = parse_ranges(args)?;
                delete_ranges(&mut project.scene.slices, &ranges, &mut next_id);
            }
            "trim" => {
                let start = args.get("start_ms").and_then(Value::as_f64).unwrap_or(0.0);
                let end = args
                    .get("end_ms")
                    .and_then(Value::as_f64)
                    .unwrap_or(project.media.duration_ms);
                trim_to_range(&mut project.scene.slices, start, end);
            }
            "auto_jump_cuts" => {
                let settings = JumpCutSettings {
                    min_silence_ms: args.get("min_silence_ms").and_then(Value::as_f64).unwrap_or(450.0),
                    padding_ms: args.get("padding_ms").and_then(Value::as_f64).unwrap_or(110.0),
                    sensitivity: args.get("sensitivity").and_then(Value::as_f64).unwrap_or(0.5),
                };
                let levels = audio_levels(&pkg, &project)?;
                let ranges = find_silences(&levels, FRAME_MS, &settings);
                clear_jump_cuts(&mut project.scene.slices);
                apply_jump_cuts(&mut project.scene.slices, &ranges, &mut next_id);
                heal(&mut project.scene.slices);
            }
            other => return Err(format!("Unsupported mode '{other}' (expected cut, restore, delete, trim, or auto_jump_cuts)")),
        }

        pkg.save_project(&project)?;
        Ok(tool_json(json!({
            "project_path": pkg.dir.to_string_lossy(),
            "mode": mode,
            "slices": project.scene.slices,
            "effective_duration_ms": effective_duration_ms(&project.scene.slices),
            "cut_duration_ms": cut_total_ms(&project.scene.slices),
        })))
    }

    fn tool_set_clip_speed(&mut self, args: &Value) -> Result<Value, String> {
        let (pkg, mut project) = resolve_package(req_str(args, "project_path")?)?;
        let speed = args
            .get("speed")
            .and_then(Value::as_f64)
            .ok_or("speed is required")?
            .clamp(0.25, 16.0);

        if let Some(id) = str_arg(args, "slice_id") {
            let sl = project
                .scene
                .slices
                .iter_mut()
                .find(|s| s.id == id)
                .ok_or_else(|| format!("Slice '{id}' not found"))?;
            sl.time_scale = speed;
        } else {
            let start = args.get("start_ms").and_then(Value::as_f64).ok_or("slice_id or start_ms/end_ms is required")?;
            let end = args.get("end_ms").and_then(Value::as_f64).ok_or("end_ms is required with start_ms")?;
            if end <= start {
                return Err("end_ms must be greater than start_ms".into());
            }
            let mut ids: Vec<String> = (0..16).map(|_| self.next_id("slice")).collect();
            let changed = set_range_speed(&mut project.scene.slices, start, end, speed, || ids.pop().unwrap_or_else(|| "slice-x".into()));
            if changed == 0 {
                return Err("No matching clip range found".into());
            }
        }
        pkg.save_project(&project)?;
        Ok(tool_json(json!({
            "project_path": pkg.dir.to_string_lossy(),
            "slices": project.scene.slices,
            "effective_duration_ms": effective_duration_ms(&project.scene.slices),
        })))
    }

    fn tool_add_zoom(&mut self, args: &Value) -> Result<Value, String> {
        let (pkg, mut project) = resolve_package(req_str(args, "project_path")?)?;
        let mode = str_arg(args, "mode").unwrap_or("follow-cursor");
        if mode == "auto" {
            let input = pkg.load_input(&project);
            let z = args
                .get("zoom")
                .and_then(Value::as_f64)
                .unwrap_or(project.config.animations.default_zoom)
                .clamp(1.0, 5.0);
            let opts = AutoZoomOptions::for_ease(z, project.config.animations.ease_in_ms);
            let mut regions = generate_auto_zoom(&input, project.media.duration_ms, &opts);
            for r in &mut regions {
                r.id = self.next_id("zoom");
            }
            let replace = args.get("replace_existing").and_then(Value::as_bool).unwrap_or(true);
            if replace {
                project.scene.zooms = regions;
            } else {
                project.scene.zooms.extend(regions);
            }
            pkg.save_project(&project)?;
            return Ok(tool_json(json!({
                "project_path": pkg.dir.to_string_lossy(),
                "mode": "auto",
                "zooms": project.scene.zooms,
            })));
        }

        let start = args.get("start_ms").and_then(Value::as_f64).unwrap_or(0.0).max(0.0);
        let max_dur = project.media.duration_ms.max(start + 400.0);
        let end = args
            .get("end_ms")
            .and_then(Value::as_f64)
            .unwrap_or_else(|| (start + 2500.0).min(max_dur));
        if end <= start + 50.0 {
            return Err("end_ms must be greater than start_ms".into());
        }
        let zoom = args
            .get("zoom")
            .and_then(Value::as_f64)
            .unwrap_or(project.config.animations.default_zoom)
            .clamp(1.0, 5.0);
        let kind = if mode == "manual" { ZoomKind::Manual } else { ZoomKind::FollowCursor };
        let x = args.get("x").and_then(Value::as_f64).unwrap_or(0.5).clamp(0.0, 1.0);
        let y = args.get("y").and_then(Value::as_f64).unwrap_or(0.5).clamp(0.0, 1.0);
        let instant = args.get("instant").and_then(Value::as_bool).unwrap_or(false);
        let id = self.next_id("zoom");
        let region = ZoomRegion {
            id: id.clone(),
            zoom,
            kind,
            manual_target_point: Point { x, y },
            is_disabled: false,
            source_start_ms: start,
            source_end_ms: end,
            instant,
            ease_in_ms: None,
            ease_out_ms: None,
        };
        project.scene.zooms.push(region.clone());
        pkg.save_project(&project)?;
        Ok(tool_json(json!({
            "project_path": pkg.dir.to_string_lossy(),
            "added": region,
            "total_zooms": project.scene.zooms.len(),
        })))
    }

    fn tool_add_annotation(&mut self, args: &Value) -> Result<Value, String> {
        let (pkg, mut project) = resolve_package(req_str(args, "project_path")?)?;
        let kind = str_arg(args, "kind").unwrap_or("text");
        if !matches!(
            kind,
            "text" | "box" | "ellipse" | "arrow" | "highlight" | "blur" | "pixelate" | "spotlight" | "image"
        ) {
            return Err(format!("Unsupported annotation kind '{kind}'"));
        }
        let start = args.get("start_ms").and_then(Value::as_f64).unwrap_or(0.0).max(0.0);
        let end = args
            .get("end_ms")
            .and_then(Value::as_f64)
            .unwrap_or_else(|| (start + 3500.0).min(project.media.duration_ms.max(start + 500.0)));
        if end <= start {
            return Err("end_ms must be greater than start_ms".into());
        }
        let id = self.next_id("annot");
        let mut a = Annotation {
            id,
            kind: kind.into(),
            space: str_arg(args, "space")
                .unwrap_or(match kind {
                    "text" | "image" => "screen",
                    _ => "content",
                })
                .into(),
            source_start_ms: start,
            source_end_ms: end,
            x: args.get("x").and_then(Value::as_f64).unwrap_or(if kind == "text" { 0.1 } else { 0.35 }),
            y: args.get("y").and_then(Value::as_f64).unwrap_or(if kind == "text" { 0.8 } else { 0.35 }),
            w: args.get("w").and_then(Value::as_f64).unwrap_or(if kind == "text" { 0.6 } else { 0.3 }),
            h: args.get("h").and_then(Value::as_f64).unwrap_or(if kind == "text" { 0.1 } else { 0.2 }),
            x2: args.get("x2").and_then(Value::as_f64).unwrap_or(0.55),
            y2: args.get("y2").and_then(Value::as_f64).unwrap_or(0.55),
            text: str_arg(args, "text").unwrap_or("Your text").into(),
            color: str_arg(args, "color")
                .unwrap_or(match kind {
                    "arrow" => "#ff453a",
                    "box" | "ellipse" => "#ffd60a",
                    "highlight" => "#ffe14a",
                    _ => "#ffffff",
                })
                .into(),
            fill: str_arg(args, "fill")
                .unwrap_or(if kind == "text" { "#000000b3" } else { "" })
                .into(),
            font_size: args.get("font_size").and_then(Value::as_f64).unwrap_or(48.0),
            thickness: args.get("thickness").and_then(Value::as_f64).unwrap_or(6.0),
            strength: args.get("strength").and_then(Value::as_f64).unwrap_or(0.65),
            anim: str_arg(args, "anim")
                .unwrap_or(match kind {
                    "blur" | "pixelate" => "none",
                    _ => "fade",
                })
                .into(),
            anim_ms: args.get("anim_ms").and_then(Value::as_f64).unwrap_or(250.0),
            image: String::new(),
            opacity: args.get("opacity").and_then(Value::as_f64).unwrap_or(1.0).clamp(0.0, 1.0),
            corner_radius: args.get("corner_radius").and_then(Value::as_f64).unwrap_or(12.0).max(0.0),
        };
        if kind == "image" {
            let src = req_str(args, "image_path")?;
            let (rel, iw, ih) = save_annotation_image(&pkg.dir, Path::new(src), 1920)?;
            a.image = rel;
            if args.get("h").is_none() {
                let (ow, oh) = output_size(&project, 1080);
                let img_aspect = iw as f64 / ih.max(1) as f64;
                let out_aspect = ow as f64 / oh.max(1) as f64;
                a.h = (a.w * out_aspect / img_aspect.max(0.05)).clamp(0.05, 0.9);
            }
        }
        project.scene.annotations.push(a.clone());
        pkg.save_project(&project)?;
        Ok(tool_json(json!({
            "project_path": pkg.dir.to_string_lossy(),
            "added": a,
            "total_annotations": project.scene.annotations.len(),
        })))
    }

    fn tool_apply_preset(&self, args: &Value) -> Result<Value, String> {
        let (pkg, mut project) = resolve_package(req_str(args, "project_path")?)?;
        if let Some(name) = str_arg(args, "preset") {
            let users = load_user_presets();
            if !find_and_apply_preset(&mut project, name, &users) {
                let names: Vec<&str> = builtin_presets().into_iter().map(|(n, _)| n).collect();
                return Err(format!("Preset '{name}' not found. Built-in presets: {}", names.join(", ")));
            }
        }
        if let Some(ar) = str_arg(args, "aspect_ratio") {
            project.config.output.aspect_ratio = ar.to_string();
        }
        if let Some(st) = str_arg(args, "subtitle_style") {
            apply_subtitle_preset(st, &mut project.config.subtitles);
        }
        if let Some(cam) = args.get("camera") {
            let mut v = serde_json::to_value(&project.config.camera).unwrap_or(Value::Null);
            deep_merge(&mut v, cam);
            if let Ok(c) = serde_json::from_value(v) {
                project.config.camera = c;
            }
        }
        pkg.save_project(&project)?;
        Ok(tool_json(json!({
            "project_path": pkg.dir.to_string_lossy(),
            "styles": project.config.styles,
            "output": project.config.output,
            "camera": project.config.camera,
            "subtitles": project.config.subtitles,
        })))
    }

    fn tool_generate_captions(&mut self, args: &Value) -> Result<Value, String> {
        let (pkg, mut project) = resolve_package(req_str(args, "project_path")?)?;
        let model = str_arg(args, "model").unwrap_or("small.en").to_string();
        let language = str_arg(args, "language").unwrap_or("auto").to_string();
        let translate = args.get("translate").and_then(Value::as_bool).unwrap_or(false);
        let replace = args.get("replace_existing").and_then(Value::as_bool).unwrap_or(true);
        let req = WhisperRequest {
            package: pkg.clone(),
            project: project.clone(),
            model,
            language,
            translate,
            cancel: Arc::new(AtomicBool::new(false)),
        };
        let caps = transcribe_sync(&req, |msg, _| {
            eprintln!("[screencraft-mcp] {msg}");
        })?;
        if replace {
            project.scene.subtitles.clear();
        }
        let count = caps.len();
        for c in caps {
            let id = self.next_id("cap");
            project.scene.subtitles.push(screencraft_core::project::Subtitle {
                id,
                source_start_ms: c.start_ms,
                source_end_ms: c.end_ms,
                text: c.text,
                word_starts: c.word_starts,
                word_ends: c.word_ends,
            });
        }
        pkg.save_project(&project)?;
        let words = extract_words(&project.scene.subtitles, &project.scene.slices);
        let text = words.iter().filter(|w| !w.cut).map(|w| w.text.as_str()).collect::<Vec<_>>().join(" ");
        Ok(tool_json(json!({
            "project_path": pkg.dir.to_string_lossy(),
            "captions_count": count,
            "word_count": words.len(),
            "transcript": text,
        })))
    }

    fn tool_render_frame(&self, args: &Value) -> Result<Value, String> {
        let (pkg, project) = resolve_package(req_str(args, "project_path")?)?;
        let timestamp_ms = args.get("timestamp_ms").and_then(Value::as_f64).unwrap_or(0.0);
        let resolution = args.get("resolution").and_then(Value::as_u64).unwrap_or(720) as u32;
        let (w, h, rgba) = render_frame_at(&pkg, &project, timestamp_ms, resolution)?;

        let mut png_bytes = Vec::new();
        {
            let mut enc = png::Encoder::new(&mut png_bytes, w, h);
            enc.set_color(png::ColorType::Rgba);
            enc.set_depth(png::BitDepth::Eight);
            enc.write_header()
                .map_err(|e| e.to_string())?
                .write_image_data(&rgba)
                .map_err(|e| e.to_string())?;
        }
        if let Some(out) = str_arg(args, "output_path") {
            let p = Path::new(out);
            if let Some(parent) = p.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            std::fs::write(p, &png_bytes).map_err(|e| e.to_string())?;
        }
        let b64 = base64::engine::general_purpose::STANDARD.encode(&png_bytes);
        let meta = json!({
            "width": w,
            "height": h,
            "timestamp_ms": timestamp_ms,
            "bytes": png_bytes.len(),
            "output_path": str_arg(args, "output_path"),
        });
        Ok(json!({
            "content": [
                { "type": "text", "text": serde_json::to_string_pretty(&meta).unwrap_or_default() },
                { "type": "image", "data": b64, "mimeType": "image/png" }
            ]
        }))
    }

    fn tool_export_video(&self, args: &Value) -> Result<Value, String> {
        let (pkg, mut project) = resolve_package(req_str(args, "project_path")?)?;
        let format = str_arg(args, "format").unwrap_or("mp4");
        if let Some(ar) = str_arg(args, "aspect_ratio") {
            project.config.output.aspect_ratio = ar.to_string();
        }
        if let Some(mb) = args.get("motion_blur").and_then(Value::as_bool) {
            project.config.animations.motion_blur = if mb { 0.8 } else { 0.0 };
        }
        let ext = match format {
            "mp4" => "mp4",
            "webm" => "webm",
            "gif" => "gif",
            other => return Err(format!("Unsupported format '{other}' (expected mp4, webm, or gif)")),
        };
        let out_path = match str_arg(args, "output_path") {
            Some(p) => PathBuf::from(p),
            None => {
                let safe: String = project.name.chars().map(|c| if "/\\:*?\"<>|".contains(c) { '-' } else { c }).collect();
                pkg.dir.parent().unwrap_or(&pkg.dir).join(format!("{safe}.{ext}"))
            }
        };
        if let Some(parent) = out_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let resolution = args.get("resolution").and_then(Value::as_u64).unwrap_or(if format == "gif" { 540 } else { 1080 }) as u32;
        if format == "gif" {
            let rep = export_gif(&pkg, &project, resolution, &out_path, |_| true)?;
            return Ok(tool_json(json!({
                "output_path": out_path.to_string_lossy(),
                "format": "gif",
                "width": rep.width,
                "height": rep.height,
                "bytes": rep.bytes,
            })));
        }

        let fps = args.get("fps").and_then(Value::as_u64).unwrap_or(60) as u32;
        let container = if format == "webm" { Container::WebM } else { Container::Mp4 };
        let codec = match str_arg(args, "codec") {
            Some("h264") => VideoCodec::H264,
            Some("av1") => VideoCodec::Av1,
            Some("vp9") => VideoCodec::Vp9,
            Some(c) => return Err(format!("Unsupported codec '{c}'")),
            None => {
                if format == "webm" {
                    if available_video_codecs().contains(&VideoCodec::Vp9) {
                        VideoCodec::Vp9
                    } else {
                        VideoCodec::Av1
                    }
                } else if available_video_codecs().contains(&VideoCodec::H264) {
                    VideoCodec::H264
                } else {
                    VideoCodec::Av1
                }
            }
        };
        let quality = str_arg(args, "quality").unwrap_or("balanced");
        let qk = match quality {
            "small" => 0.5,
            "balanced" => 1.0,
            "high" => 1.8,
            "max" => 3.0,
            other => return Err(format!("Unsupported quality '{other}' (expected small, balanced, high, or max)")),
        };
        let (w, h) = output_size(&project, resolution);
        let bitrate_kbps = Some((default_bitrate_kbps(w, h, fps) as f64 * qk).round() as u32);
        let audio = args.get("audio").and_then(Value::as_bool).unwrap_or(true);
        let opts = ExportOptions {
            container,
            codec,
            resolution,
            fps,
            bitrate_kbps,
            effort: if quality == "small" { Effort::Best } else { Effort::Balanced },
            hardware: HwPreference::Auto,
            audio,
            threads: 0,
        };
        let rep = export_project(&pkg, &project, &opts, &out_path, |_| true)?;
        Ok(tool_json(json!({
            "output_path": out_path.to_string_lossy(),
            "format": format,
            "codec": rep.summary.encoder,
            "width": rep.width,
            "height": rep.height,
            "frames": rep.frames,
            "bytes": rep.summary.bytes,
        })))
    }
}

fn default_recordings_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("SCREENCRAFT_RECORDINGS_DIR") {
        return dir.into();
    }
    if let Some(cfg) = dirs::config_dir() {
        let p = cfg.join("ScreenCraft").join("settings.json");
        if let Ok(bytes) = std::fs::read(p) {
            if let Ok(v) = serde_json::from_slice::<Value>(&bytes) {
                if let Some(d) = v.get("recordings_dir").and_then(Value::as_str) {
                    return PathBuf::from(d);
                }
            }
        }
    }
    dirs::video_dir()
        .or_else(dirs::home_dir)
        .unwrap_or_else(|| PathBuf::from("."))
        .join("ScreenCraft")
}

fn load_user_presets() -> Vec<(String, Value)> {
    dirs::config_dir()
        .map(|d| d.join("ScreenCraft").join("presets.json"))
        .and_then(|p| std::fs::read(p).ok())
        .and_then(|b| serde_json::from_slice::<serde_json::Map<String, Value>>(&b).ok())
        .map(|m| m.into_iter().collect())
        .unwrap_or_default()
}

fn local_stamp() -> String {
    if let Some((y, m, d, h, min, s)) = local_time() {
        return format!("{y:04}-{m:02}-{d:02} {h:02}-{min:02}-{s:02}");
    }
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("rec-{secs}")
}

fn resolve_package(path: &str) -> Result<(Package, Project), String> {
    let p = Path::new(path);
    let dir = if p.is_file() {
        p.parent().ok_or_else(|| format!("Invalid project path: {path}"))?.to_path_buf()
    } else {
        p.to_path_buf()
    };
    let pkg = Package::open(dir);
    let proj = pkg.load_project()?;
    Ok((pkg, proj))
}

fn save_annotation_image(package_dir: &Path, src: &Path, max_width: u32) -> Result<(String, u32, u32), String> {
    let img = image::open(src).map_err(|e| format!("Could not open image {}: {e}", src.display()))?;
    let img = if img.width() > max_width {
        let h = (img.height() as f64 * max_width as f64 / img.width() as f64).round().max(1.0) as u32;
        img.resize_exact(max_width, h, image::imageops::FilterType::Lanczos3)
    } else {
        img
    };
    let (w, h) = (img.width().max(1), img.height().max(1));
    let mut bytes = Vec::new();
    img.write_to(&mut std::io::Cursor::new(&mut bytes), image::ImageFormat::Png)
        .map_err(|e| e.to_string())?;
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    bytes.hash(&mut hasher);
    let name = format!("annot-{:016x}.png", hasher.finish());
    let dest = package_dir.join(&name);
    if !dest.exists() {
        std::fs::write(&dest, &bytes).map_err(|e| format!("Could not save image into package: {e}"))?;
    }
    Ok((name, w, h))
}

fn parse_ranges(args: &Value) -> Result<Vec<[f64; 2]>, String> {
    if let Some(arr) = args.get("ranges").and_then(Value::as_array) {
        let mut out = Vec::with_capacity(arr.len());
        for item in arr {
            if let Some(pair) = item.as_array() {
                if pair.len() == 2 {
                    if let (Some(a), Some(b)) = (pair[0].as_f64(), pair[1].as_f64()) {
                        if b > a {
                            out.push([a, b]);
                        }
                        continue;
                    }
                }
            }
            if let (Some(a), Some(b)) = (item.get("start_ms").and_then(Value::as_f64), item.get("end_ms").and_then(Value::as_f64)) {
                if b > a {
                    out.push([a, b]);
                }
            }
        }
        if out.is_empty() {
            return Err("ranges must contain at least one [start_ms, end_ms] range with end_ms > start_ms".into());
        }
        return Ok(out);
    }
    let a = args.get("start_ms").and_then(Value::as_f64).ok_or("start_ms (or ranges) is required")?;
    let b = args.get("end_ms").and_then(Value::as_f64).ok_or("end_ms (or ranges) is required")?;
    if b <= a {
        return Err("end_ms must be greater than start_ms".into());
    }
    Ok(vec![[a, b]])
}

fn delete_ranges(slices: &mut Vec<Slice>, ranges: &[[f64; 2]], mut new_id: impl FnMut() -> String) {
    for &[a, b] in ranges {
        let mut next = Vec::with_capacity(slices.len() + 1);
        for s in slices.drain(..) {
            if s.source_end_ms <= a || s.source_start_ms >= b {
                next.push(s);
                continue;
            }
            if s.source_start_ms < a - 1.0 {
                let mut left = s.clone();
                left.source_end_ms = a;
                next.push(left);
            }
            if s.source_end_ms > b + 1.0 {
                let mut right = s.clone();
                if s.source_start_ms < a - 1.0 {
                    right.id = new_id();
                    right.transition = None;
                }
                right.source_start_ms = b;
                next.push(right);
            }
        }
        *slices = next;
    }
}

fn trim_to_range(slices: &mut Vec<Slice>, start_ms: f64, end_ms: f64) {
    if end_ms <= start_ms {
        return;
    }
    let mut next = Vec::with_capacity(slices.len());
    for mut s in slices.drain(..) {
        let lo = s.source_start_ms.max(start_ms);
        let hi = s.source_end_ms.min(end_ms);
        if hi - lo >= 1.0 {
            s.source_start_ms = lo;
            s.source_end_ms = hi;
            next.push(s);
        }
    }
    *slices = next;
}

fn set_range_speed(slices: &mut Vec<Slice>, a: f64, b: f64, speed: f64, mut new_id: impl FnMut() -> String) -> usize {
    let mut count = 0;
    let cur = std::mem::take(slices);
    let mut next = Vec::with_capacity(cur.len() + 2);
    for s in cur {
        if s.freeze || s.source_end_ms <= a || s.source_start_ms >= b {
            next.push(s);
            continue;
        }
        let (lo, hi) = (a.max(s.source_start_ms), b.min(s.source_end_ms));
        if hi - lo < 10.0 {
            next.push(s);
            continue;
        }
        count += 1;
        let mut first = true;
        if lo > s.source_start_ms + 1.0 {
            let mut left = s.clone();
            left.source_end_ms = lo;
            next.push(left);
            first = false;
        }
        let mut mid = s.clone();
        if !first {
            mid.id = new_id();
            mid.transition = None;
        }
        mid.source_start_ms = lo;
        mid.source_end_ms = hi;
        mid.time_scale = speed;
        next.push(mid);
        if hi < s.source_end_ms - 1.0 {
            let mut right = s.clone();
            right.id = new_id();
            right.source_start_ms = hi;
            right.transition = None;
            next.push(right);
        }
    }
    *slices = next;
    count
}

fn str_arg<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key).and_then(Value::as_str).filter(|s| !s.is_empty())
}

fn req_str<'a>(args: &'a Value, key: &str) -> Result<&'a str, String> {
    str_arg(args, key).ok_or_else(|| format!("Missing required parameter '{key}'"))
}

fn tool_json(val: Value) -> Value {
    json!({
        "content": [{
            "type": "text",
            "text": serde_json::to_string_pretty(&val).unwrap_or_default()
        }]
    })
}

fn tool_error(msg: String) -> Value {
    json!({
        "content": [{ "type": "text", "text": msg }],
        "isError": true
    })
}

fn rpc_error(id: Value, code: i64, message: String) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": { "code": code, "message": message }
    })
}

fn tool_definitions() -> Value {
    json!([
        {
            "name": "list_projects",
            "description": "List ScreenCraft .screencraft project packages in the recordings directory (newest first).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "directory": { "type": "string", "description": "Optional folder to scan instead of the default ScreenCraft recordings directory." }
                }
            }
        },
        {
            "name": "get_project_summary",
            "description": "Inspect a .screencraft project: duration, slices, zooms, annotations, captions, word-level transcript, and recorded click/shortcut timestamps.",
            "inputSchema": {
                "type": "object",
                "required": ["project_path"],
                "properties": {
                    "project_path": { "type": "string", "description": "Path to the .screencraft folder or its project.json." }
                }
            }
        },
        {
            "name": "list_capture_sources",
            "description": "List available displays, windows, and cameras that can be recorded.",
            "inputSchema": { "type": "object", "properties": {} }
        },
        {
            "name": "start_recording",
            "description": "Start recording a display, window, screen area, or webcam-only camera feed into a new .screencraft package.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "target": { "type": "string", "enum": ["display", "window", "area", "camera"], "default": "display" },
                    "display_id": { "type": "integer" },
                    "window_id": { "type": "integer" },
                    "area": {
                        "type": "object",
                        "properties": {
                            "x": { "type": "number" },
                            "y": { "type": "number" },
                            "width": { "type": "number" },
                            "height": { "type": "number" }
                        },
                        "required": ["x", "y", "width", "height"]
                    },
                    "camera": { "type": "boolean", "default": false, "description": "Also capture a camera bubble when target is display/window/area (always true for target='camera')." },
                    "camera_id": { "type": "string", "description": "Optional camera id from list_capture_sources (defaults to the first webcam)." },
                    "vertical": { "type": "boolean", "default": false, "description": "Crop landscape webcam feed to 9:16 portrait when target is 'camera'." },
                    "fps": { "type": "integer", "default": 30 },
                    "max_long_side": { "type": "integer", "default": 2560 },
                    "system_audio": { "type": "boolean", "default": true },
                    "microphone": { "type": "boolean", "default": true },
                    "microphone_id": { "type": "string" },
                    "output_dir": { "type": "string" },
                    "name": { "type": "string" }
                }
            }
        },
        {
            "name": "stop_recording",
            "description": "Stop the active recording, flush files, and optionally run auto-zoom (screen) or auto-jump-cuts.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "auto_zoom": { "type": "boolean", "default": true },
                    "auto_jump_cuts": { "type": "boolean", "default": false }
                }
            }
        },
        {
            "name": "trim_or_cut",
            "description": "Cut, restore, delete, trim, or auto-jump-cut silent stretches on the timeline.",
            "inputSchema": {
                "type": "object",
                "required": ["project_path"],
                "properties": {
                    "project_path": { "type": "string" },
                    "mode": { "type": "string", "enum": ["cut", "restore", "delete", "trim", "auto_jump_cuts"], "default": "cut" },
                    "start_ms": { "type": "number" },
                    "end_ms": { "type": "number" },
                    "ranges": { "type": "array", "items": { "type": "array", "items": { "type": "number" }, "minItems": 2, "maxItems": 2 } },
                    "min_silence_ms": { "type": "number", "default": 450.0 },
                    "padding_ms": { "type": "number", "default": 110.0 },
                    "sensitivity": { "type": "number", "default": 0.5 }
                }
            }
        },
        {
            "name": "set_clip_speed",
            "description": "Set playback speed (0.25x to 16x) for a slice ID or a source time range.",
            "inputSchema": {
                "type": "object",
                "required": ["project_path", "speed"],
                "properties": {
                    "project_path": { "type": "string" },
                    "slice_id": { "type": "string" },
                    "start_ms": { "type": "number" },
                    "end_ms": { "type": "number" },
                    "speed": { "type": "number" }
                }
            }
        },
        {
            "name": "add_zoom",
            "description": "Add a zoom region (follow-cursor or manual fixed point) or regenerate auto-zooms from recorded clicks and typing.",
            "inputSchema": {
                "type": "object",
                "required": ["project_path"],
                "properties": {
                    "project_path": { "type": "string" },
                    "mode": { "type": "string", "enum": ["follow-cursor", "manual", "auto"], "default": "follow-cursor" },
                    "start_ms": { "type": "number" },
                    "end_ms": { "type": "number" },
                    "zoom": { "type": "number", "default": 1.6 },
                    "x": { "type": "number", "default": 0.5 },
                    "y": { "type": "number", "default": 0.5 },
                    "instant": { "type": "boolean", "default": false },
                    "replace_existing": { "type": "boolean", "default": true }
                }
            }
        },
        {
            "name": "add_annotation",
            "description": "Add a timed annotation (text, arrow, box, ellipse, highlight, blur, pixelate, spotlight, or image) to the project.",
            "inputSchema": {
                "type": "object",
                "required": ["project_path", "kind"],
                "properties": {
                    "project_path": { "type": "string" },
                    "kind": { "type": "string", "enum": ["text", "box", "ellipse", "arrow", "highlight", "blur", "pixelate", "spotlight", "image"] },
                    "start_ms": { "type": "number", "default": 0.0 },
                    "end_ms": { "type": "number" },
                    "space": { "type": "string", "enum": ["content", "screen"] },
                    "x": { "type": "number" },
                    "y": { "type": "number" },
                    "w": { "type": "number" },
                    "h": { "type": "number" },
                    "x2": { "type": "number" },
                    "y2": { "type": "number" },
                    "text": { "type": "string" },
                    "color": { "type": "string" },
                    "fill": { "type": "string" },
                    "font_size": { "type": "number" },
                    "thickness": { "type": "number" },
                    "strength": { "type": "number" },
                    "anim": { "type": "string", "enum": ["none", "fade", "pop", "slide"] },
                    "anim_ms": { "type": "number" },
                    "image_path": { "type": "string", "description": "Local image path to copy into the .screencraft package when kind is 'image'." },
                    "opacity": { "type": "number", "default": 1.0 },
                    "corner_radius": { "type": "number", "default": 12.0 }
                }
            }
        },
        {
            "name": "apply_preset",
            "description": "Apply a built-in or user style preset, aspect ratio, subtitle style, or partial camera overrides.",
            "inputSchema": {
                "type": "object",
                "required": ["project_path"],
                "properties": {
                    "project_path": { "type": "string" },
                    "preset": { "type": "string", "description": "Preset name (Clean, Minimal, Vivid, Sunset, or a saved user preset)." },
                    "aspect_ratio": { "type": "string", "enum": ["auto", "16:9", "9:16", "1:1", "4:3", "3:4", "4:5", "21:9"] },
                    "subtitle_style": { "type": "string", "enum": ["classic", "minimal", "tiktok"] },
                    "camera": { "type": "object" }
                }
            }
        },
        {
            "name": "generate_captions",
            "description": "Run local whisper.cpp speech recognition on the recording to generate word-timed captions and transcript.",
            "inputSchema": {
                "type": "object",
                "required": ["project_path"],
                "properties": {
                    "project_path": { "type": "string" },
                    "model": { "type": "string", "default": "small.en" },
                    "language": { "type": "string", "default": "auto" },
                    "translate": { "type": "boolean", "default": false },
                    "replace_existing": { "type": "boolean", "default": true }
                }
            }
        },
        {
            "name": "render_frame",
            "description": "Render a single preview PNG frame at output timestamp_ms for visual verification.",
            "inputSchema": {
                "type": "object",
                "required": ["project_path"],
                "properties": {
                    "project_path": { "type": "string" },
                    "timestamp_ms": { "type": "number", "default": 0.0 },
                    "resolution": { "type": "integer", "default": 720 },
                    "output_path": { "type": "string" }
                }
            }
        },
        {
            "name": "export_video",
            "description": "Headlessly export the edited project to MP4, WebM, or GIF.",
            "inputSchema": {
                "type": "object",
                "required": ["project_path"],
                "properties": {
                    "project_path": { "type": "string" },
                    "output_path": { "type": "string" },
                    "format": { "type": "string", "enum": ["mp4", "webm", "gif"], "default": "mp4" },
                    "codec": { "type": "string", "enum": ["h264", "av1", "vp9"] },
                    "resolution": { "type": "integer", "default": 1080 },
                    "fps": { "type": "integer", "default": 60 },
                    "quality": { "type": "string", "enum": ["small", "balanced", "high", "max"], "default": "balanced" },
                    "motion_blur": { "type": "boolean" },
                    "audio": { "type": "boolean", "default": true },
                    "aspect_ratio": { "type": "string" }
                }
            }
        }
    ])
}
