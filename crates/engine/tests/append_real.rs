//! Manual check: join two real recordings. Set SC_APPEND_TARGET (a copy of a `.screencraft`
//! folder, it is changed) and SC_APPEND_EXTRA, then run with `--ignored`.

#[test]
#[ignore]
fn append_two_recordings() {
    let (Ok(t), Ok(e)) = (std::env::var("SC_APPEND_TARGET"), std::env::var("SC_APPEND_EXTRA")) else { return };
    let before = screencraft_engine::Package::open(&t).load_project().unwrap();
    let a = screencraft_engine::append::append(t.as_ref(), e.as_ref(), |_| true).unwrap();
    let after = screencraft_engine::Package::open(&t).load_project().unwrap();
    println!("added {} ms; {} -> {} ms; slices {}; screen {}", a.added_ms, before.media.duration_ms, after.media.duration_ms, after.scene.slices.len(), after.media.screen);
    assert!(after.media.duration_ms > before.media.duration_ms);
    assert_eq!(after.scene.slices.len(), before.scene.slices.len() + 1);
}

fn make_pkg(dir: &std::path::Path, frames: u32) -> screencraft_engine::Package {
    use screencraft_core::project::{Media, Project, Rect, Slice};
    use screencraft_encode::color::{to_i420, PixelOrder};
    use screencraft_encode::{Container, Effort, ExportSettings, Exporter, FileSink, HwPreference, Sink, VideoCodec, VideoConfig};

    let pkg = screencraft_engine::Package::open(dir.to_path_buf());
    std::fs::create_dir_all(&pkg.dir).unwrap();
    let (w, h, fps) = (320u32, 180u32, 30u32);
    let mut video = VideoConfig::new(VideoCodec::H264, w, h, fps);
    video.hardware = HwPreference::Software;
    video.effort = Effort::Fast;
    let mut ex = Exporter::new(ExportSettings { container: Container::Mp4, video, audio: vec![] }).unwrap();
    let mut sink = FileSink::create(&pkg.dir.join("screen.mp4")).unwrap();
    for f in 0..frames {
        let px = vec![128u8; (w * h * 4) as usize];
        ex.push_video(&to_i420(&px, w, h, (w * 4) as usize, PixelOrder::Rgba, 1), (f as u64 * 1_000_000 / fps as u64) as i64).unwrap();
        while let Some(op) = ex.poll() {
            sink.apply(&op).unwrap();
        }
    }
    let s = ex.finish().unwrap();
    while let Some(op) = ex.poll() {
        sink.apply(&op).unwrap();
    }
    let dur = s.duration_us as f64 / 1000.0;
    let media = Media {
        duration_ms: dur,
        width: w,
        height: h,
        bounds: Rect { x: 0.0, y: 0.0, width: w as f64, height: h as f64 },
        ..Default::default()
    };
    let mut proj = Project::from_recording("Append test", media);
    proj.scene.slices = vec![
        Slice { id: "s0".into(), source_start_ms: 0.0, source_end_ms: dur / 2.0, ..Default::default() },
        Slice { id: "s1".into(), source_start_ms: dur / 2.0, source_end_ms: dur, ..Default::default() },
    ];
    pkg.save_project(&proj).unwrap();
    std::fs::write(pkg.dir.join("input.json"), b"{}").unwrap();
    pkg
}

#[test]
fn append_at_inserts_slice_at_requested_index() {
    let dir = std::env::temp_dir().join(format!("sc-append-at-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let target = make_pkg(&dir.join("target.screencraft"), 30);
    let extra = make_pkg(&dir.join("extra.screencraft"), 45);
    // Add clicks to `extra` so auto-zoom generates regions for the appended segment.
    std::fs::write(
        extra.dir.join("input.json"),
        serde_json::to_vec(&screencraft_core::input::InputLog {
            clicks: vec![screencraft_core::input::ClickEvent {
                t: 500.0,
                x: 160.0,
                y: 90.0,
                button: screencraft_core::input::Button::Left,
                down: true,
            }],
            ..Default::default()
        })
        .unwrap(),
    )
    .unwrap();

    let before = target.load_project().unwrap();
    let res = screencraft_engine::append::append_at(&target.dir, &extra.dir, Some(1), |_| true).unwrap();
    let after = target.load_project().unwrap();

    assert_eq!(after.media.segments.len(), 2);
    assert_eq!(after.scene.slices.len(), 3);
    assert_eq!(after.scene.slices[0].id, "s0");
    assert_eq!(after.scene.slices[1].id, res.slice_id);
    assert_eq!(after.scene.slices[2].id, "s1");
    assert!(after.scene.slices[1].source_start_ms >= before.media.duration_ms - 1.0);
    assert!(after.scene.slices[1].source_end_ms > after.scene.slices[1].source_start_ms);

    // Auto-zoom should have generated at least one region inside the newly added segment range.
    assert!(
        after.scene.zooms.iter().any(|z| z.source_start_ms >= before.media.duration_ms - 1.0 && z.source_end_ms <= after.media.duration_ms + 1.0),
        "expected auto-zoom region for appended segment, got {:?}",
        after.scene.zooms
    );

    // MultiSource should decode frames across both segments.
    let mut src = screencraft_engine::MultiSource::open(&target, &after).unwrap();
    assert!(src.fetch(100.0, None).is_ok());
    assert!(src.fetch(before.media.duration_ms + 100.0, None).is_ok());

    let _ = std::fs::remove_dir_all(&dir);
}
