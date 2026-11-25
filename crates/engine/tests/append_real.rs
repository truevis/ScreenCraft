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
