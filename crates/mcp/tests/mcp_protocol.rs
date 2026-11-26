//! End-to-end JSON-RPC protocol & tool tests for `screencraft-mcp`.

use base64::Engine;
use screencraft_core::input::{Button, ClickEvent, InputLog};
use screencraft_core::project::{Media, Project, Rect, Subtitle};
use screencraft_encode::color::{to_i420, PixelOrder};
use screencraft_encode::{Container, Effort, ExportSettings, Exporter, FileSink, HwPreference, Sink, VideoCodec, VideoConfig};
use screencraft_engine::Package;
use screencraft_mcp::McpServer;
use serde_json::{json, Value};

fn make_package(dir: &std::path::Path) -> Package {
    let package = Package::open(dir.join("Demo.screencraft"));
    std::fs::create_dir_all(&package.dir).unwrap();
    let (w, h, fps, frames) = (320u32, 180u32, 30u32, 30u32);
    let mut video = VideoConfig::new(VideoCodec::H264, w, h, fps);
    video.hardware = HwPreference::Software;
    video.effort = Effort::Fast;
    let mut ex = Exporter::new(ExportSettings { container: Container::Mp4, video, audio: vec![] }).unwrap();
    let mut sink = FileSink::create(&package.dir.join("screen.mp4")).unwrap();
    for f in 0..frames {
        let mut px = vec![255u8; (w * h * 4) as usize];
        for y in 0..h {
            for x in 0..w {
                let o = ((y * w + x) * 4) as usize;
                px[o] = (x * 255 / w) as u8;
                px[o + 1] = (y * 255 / h) as u8;
                px[o + 2] = (f * 8) as u8;
            }
        }
        ex.push_video(&to_i420(&px, w, h, (w * 4) as usize, PixelOrder::Rgba, 1), (f as u64 * 1_000_000 / fps as u64) as i64)
            .unwrap();
        while let Some(op) = ex.poll() {
            sink.apply(&op).unwrap();
        }
    }
    let s = ex.finish().unwrap();
    while let Some(op) = ex.poll() {
        sink.apply(&op).unwrap();
    }
    let media = Media {
        duration_ms: s.duration_us as f64 / 1000.0,
        width: w,
        height: h,
        bounds: Rect { x: 0.0, y: 0.0, width: w as f64, height: h as f64 },
        ..Default::default()
    };
    let mut proj = Project::from_recording("MCP Demo", media);
    proj.scene.subtitles.push(Subtitle {
        id: "cap-0".into(),
        source_start_ms: 100.0,
        source_end_ms: 900.0,
        text: "Hello um world".into(),
        word_starts: vec![100.0, 350.0, 600.0],
        word_ends: vec![300.0, 550.0, 900.0],
    });
    package.save_project(&proj).unwrap();

    let log = InputLog {
        bounds: Rect { x: 0.0, y: 0.0, width: w as f64, height: h as f64 },
        clicks: vec![ClickEvent { t: 400.0, x: 160.0, y: 90.0, button: Button::Left, down: true }],
        ..Default::default()
    };
    std::fs::write(package.dir.join("input.json"), serde_json::to_vec(&log).unwrap()).unwrap();
    package
}

fn rpc(server: &mut McpServer, req: Value) -> Value {
    let raw = serde_json::to_string(&req).unwrap();
    server.handle_message(&raw).expect("expected JSON-RPC response")
}

fn call_tool(server: &mut McpServer, id: u64, name: &str, args: Value) -> (bool, Value) {
    let resp = rpc(
        server,
        json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "tools/call",
            "params": { "name": name, "arguments": args }
        }),
    );
    let res = resp.get("result").cloned().expect("expected result");
    let is_err = res.get("isError").and_then(Value::as_bool).unwrap_or(false);
    (is_err, res)
}

fn parse_tool_json(res: &Value) -> Value {
    let txt = res["content"][0]["text"].as_str().expect("text content");
    serde_json::from_str(txt).expect("valid JSON output")
}

#[test]
fn mcp_protocol_and_editing_roundtrip() {
    let dir = std::env::temp_dir().join(format!("sc-mcp-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let pkg = make_package(&dir);

    let badge_path = dir.join("badge.png");
    image::RgbaImage::from_pixel(32, 16, image::Rgba([40, 180, 250, 255]))
        .save(&badge_path)
        .unwrap();

    let mut server = McpServer::with_recordings_dir(dir.clone());

    // 1. Initialize & notification & ping
    let init = rpc(&mut server, json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": { "protocolVersion": "2024-11-05" } }));
    assert_eq!(init["result"]["serverInfo"]["name"], "screencraft-mcp");
    assert!(server.handle_message(&json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }).to_string()).is_none());
    let ping = rpc(&mut server, json!({ "jsonrpc": "2.0", "id": 2, "method": "ping" }));
    assert!(ping.get("result").is_some());

    // 2. tools/list
    let tools_resp = rpc(&mut server, json!({ "jsonrpc": "2.0", "id": 3, "method": "tools/list" }));
    let tools = tools_resp["result"]["tools"].as_array().unwrap();
    assert_eq!(tools.len(), 13);
    for expected in [
        "list_projects",
        "get_project_summary",
        "list_capture_sources",
        "start_recording",
        "stop_recording",
        "trim_or_cut",
        "set_clip_speed",
        "add_zoom",
        "add_annotation",
        "apply_preset",
        "generate_captions",
        "render_frame",
        "export_video",
    ] {
        assert!(tools.iter().any(|t| t["name"] == expected), "missing tool {expected}");
    }
    let start_tool = tools.iter().find(|t| t["name"] == "start_recording").unwrap();
    let targets = start_tool["inputSchema"]["properties"]["target"]["enum"].as_array().unwrap();
    assert!(targets.iter().any(|v| v == "camera"), "start_recording missing camera target");
    assert!(start_tool["inputSchema"]["properties"].get("camera_id").is_some());
    assert!(start_tool["inputSchema"]["properties"].get("vertical").is_some());

    // 3. list_projects & get_project_summary
    let (err, res) = call_tool(&mut server, 4, "list_projects", json!({}));
    assert!(!err);
    let listed = parse_tool_json(&res);
    assert_eq!(listed["projects"].as_array().unwrap().len(), 1);

    let proj_str = pkg.dir.to_string_lossy().into_owned();
    let (err, res) = call_tool(&mut server, 5, "get_project_summary", json!({ "project_path": proj_str }));
    assert!(!err);
    let summary = parse_tool_json(&res);
    assert_eq!(summary["transcript"]["text"], "Hello um world");

    // 4. add_zoom (manual + auto)
    let (err, _) = call_tool(
        &mut server,
        6,
        "add_zoom",
        json!({ "project_path": proj_str, "mode": "manual", "start_ms": 100.0, "end_ms": 800.0, "zoom": 1.8, "x": 0.4, "y": 0.6 }),
    );
    assert!(!err);

    // 5. add_annotation (text with slide animation + image overlay copied into package)
    let (err, _) = call_tool(
        &mut server,
        7,
        "add_annotation",
        json!({
            "project_path": proj_str,
            "kind": "text",
            "text": "Step 1",
            "start_ms": 50.0,
            "end_ms": 900.0,
            "anim": "slide"
        }),
    );
    assert!(!err);
    let (err, res) = call_tool(
        &mut server,
        8,
        "add_annotation",
        json!({
            "project_path": proj_str,
            "kind": "image",
            "image_path": badge_path.to_string_lossy(),
            "start_ms": 50.0,
            "end_ms": 900.0,
            "opacity": 0.9,
            "corner_radius": 8.0
        }),
    );
    assert!(!err, "add_annotation image failed: {res}");
    let annot_json = parse_tool_json(&res);
    let copied_name = annot_json["added"]["image"].as_str().unwrap();
    assert!(pkg.dir.join(copied_name).exists());

    // 6. apply_preset
    let (err, res) = call_tool(
        &mut server,
        9,
        "apply_preset",
        json!({
            "project_path": proj_str,
            "preset": "Clean",
            "aspect_ratio": "9:16",
            "subtitle_style": "tiktok",
            "camera": { "mirror": false, "shadowIntensity": 0.8 }
        }),
    );
    assert!(!err, "apply_preset failed: {res}");
    let p_after = pkg.load_project().unwrap();
    assert_eq!(p_after.config.output.aspect_ratio, "9:16");
    assert_eq!(p_after.config.subtitles.style, "tiktok");
    assert!(!p_after.config.camera.mirror);
    assert_eq!(p_after.config.camera.shadow_intensity, 0.8);

    // 7. trim_or_cut (cut "um" -> restore "um") and set_clip_speed
    let (err, res) = call_tool(
        &mut server,
        10,
        "trim_or_cut",
        json!({ "project_path": proj_str, "mode": "cut", "start_ms": 350.0, "end_ms": 550.0 }),
    );
    assert!(!err, "cut failed: {res}");
    let (_, sum_after_cut) = call_tool(&mut server, 11, "get_project_summary", json!({ "project_path": proj_str }));
    assert_eq!(parse_tool_json(&sum_after_cut)["transcript"]["text"], "Hello world");

    let (err, res) = call_tool(
        &mut server,
        12,
        "trim_or_cut",
        json!({ "project_path": proj_str, "mode": "restore", "start_ms": 350.0, "end_ms": 550.0 }),
    );
    assert!(!err, "restore failed: {res}");
    let (_, sum_after_restore) = call_tool(&mut server, 13, "get_project_summary", json!({ "project_path": proj_str }));
    assert_eq!(parse_tool_json(&sum_after_restore)["transcript"]["text"], "Hello um world");

    let (err, res) = call_tool(
        &mut server,
        14,
        "set_clip_speed",
        json!({ "project_path": proj_str, "start_ms": 200.0, "end_ms": 600.0, "speed": 2.0 }),
    );
    assert!(!err, "set_clip_speed failed: {res}");

    // Restore auto aspect ratio before render/export
    let (err, _) = call_tool(
        &mut server,
        15,
        "apply_preset",
        json!({ "project_path": proj_str, "aspect_ratio": "auto" }),
    );
    assert!(!err);

    // 8. render_frame + export_video (gracefully skip only if no GPU adapter is present)
    let frame_png = dir.join("frame.png");
    let (err, res) = call_tool(
        &mut server,
        16,
        "render_frame",
        json!({ "project_path": proj_str, "timestamp_ms": 200.0, "resolution": 360, "output_path": frame_png.to_string_lossy() }),
    );
    if err && res["content"][0]["text"].as_str().unwrap_or("").contains("no GPU adapter") {
        eprintln!("skipping GPU render/export checks: no GPU adapter");
        let _ = std::fs::remove_dir_all(&dir);
        return;
    }
    assert!(!err, "render_frame failed: {res}");
    assert_eq!(res["content"][1]["type"], "image");
    assert_eq!(res["content"][1]["mimeType"], "image/png");
    let png_decoded = base64::engine::general_purpose::STANDARD
        .decode(res["content"][1]["data"].as_str().unwrap())
        .unwrap();
    assert_eq!(&png_decoded[..4], b"\x89PNG");
    assert!(frame_png.exists());

    let out_mp4 = dir.join("export.mp4");
    let (err, res) = call_tool(
        &mut server,
        17,
        "export_video",
        json!({
            "project_path": proj_str,
            "output_path": out_mp4.to_string_lossy(),
            "format": "mp4",
            "resolution": 360,
            "fps": 30,
            "motion_blur": false,
            "audio": false
        }),
    );
    assert!(!err, "export_video mp4 failed: {res}");
    assert!(std::fs::metadata(&out_mp4).unwrap().len() > 200);

    let out_gif = dir.join("export.gif");
    let (err, res) = call_tool(
        &mut server,
        18,
        "export_video",
        json!({
            "project_path": proj_str,
            "output_path": out_gif.to_string_lossy(),
            "format": "gif",
            "resolution": 240
        }),
    );
    assert!(!err, "export_video gif failed: {res}");
    assert!(std::fs::metadata(&out_gif).unwrap().len() > 100);

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn mcp_webcam_only_project_and_validation() {
    let dir = std::env::temp_dir().join(format!("sc-mcp-cam-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let pkg = make_package(&dir);
    let mut proj = pkg.load_project().unwrap();
    proj = Project::talking_head(&proj.name, proj.media.clone());
    pkg.save_project(&proj).unwrap();

    let mut server = McpServer::with_recordings_dir(dir.clone());
    let (err, res) = call_tool(&mut server, 1, "list_projects", json!({}));
    assert!(!err);
    let listed = parse_tool_json(&res);
    assert_eq!(listed["projects"][0]["camera_only"], true);
    assert_eq!(listed["projects"][0]["has_camera"], true);

    let proj_str = pkg.dir.to_string_lossy().into_owned();
    let (err, res) = call_tool(&mut server, 2, "get_project_summary", json!({ "project_path": proj_str }));
    assert!(!err);
    assert_eq!(parse_tool_json(&res)["media"]["cameraOnly"], true);

    let (err, res) = call_tool(
        &mut server,
        3,
        "render_frame",
        json!({ "project_path": proj_str, "timestamp_ms": 200.0, "resolution": 360 }),
    );
    if !err || !res["content"][0]["text"].as_str().unwrap_or("").contains("no GPU adapter") {
        assert!(!err, "render_frame on talking_head failed: {res}");
        assert_eq!(res["content"][1]["type"], "image");
        let png_decoded = base64::engine::general_purpose::STANDARD
            .decode(res["content"][1]["data"].as_str().unwrap())
            .unwrap();
        assert_eq!(&png_decoded[..4], b"\x89PNG");
    }

    let (err, res) = call_tool(
        &mut server,
        4,
        "start_recording",
        json!({ "target": "camera", "camera_id": "9999", "vertical": true }),
    );
    assert!(err, "invalid camera_id should fail cleanly: {res}");

    let _ = std::fs::remove_dir_all(&dir);
}
