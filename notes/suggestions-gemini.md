# ScreenCraft: Elegance, UX/UI, and Efficiency Review

Scope: Windows application (`crates/app`, `crates/render`, `crates/engine`, `crates/core`) on the current workspace (`0.4.1`). This complements [windows-code-review.md](windows-code-review.md) (which covers Media Foundation encoding, DXGI/WGC capture, and packaging) and focuses on **UX & UI**, **efficiency**, and **code/architectural elegance**.

---

## 1. UX & UI Improvements

### Quick fixes (visible bugs & platform leaks)

- **Fix UTF-8 mojibake in visible UI strings.** Several files contain double-encoded characters (`Â·`, `Ã—`, `â€”`, `â†’`, `â€¦`) that render literally to the user:
  - `crates/app/src/area.rs:102, 113` (area dimensions badge and top banner).
  - `crates/app/src/compressor.rs:22-27, 120, 164, 173, 218, 227, 238, 253` (preset subtitles, file metadata, progress text, and buttons).
- **Replace remaining macOS copy and shortcut hints with Windows conventions:**
  - `crates/app/src/editor/mod.rs:1275, 1278`: `"Redo (⇧⌘Z)"` / `"Undo (⌘Z)"` → `"Redo (Ctrl+Shift+Z / Ctrl+Y)"` and `"Undo (Ctrl+Z)"`, and bind `Ctrl+Y` to redo in `handle_keys` (`mod.rs:1029`).
  - `crates/app/src/editor/timeline.rs:132, 135`: `"Duplicate selected clip (⌘D)"` / `"Delete selected (⌫)"` → `Ctrl+D` and `Del`.
  - `crates/app/src/editor/panels/annotate.rs:77` & `captions.rs:142`: replace `⌘C` / `⌘ / Ctrl-click` with `Ctrl+C` / `Ctrl-click`.
  - `crates/app/src/recorder.rs:510`: `"{} × {} on Retina"` hardcodes `* 2` for logical display dimensions. On Windows, DPI scaling can be 100%, 125%, 150%, 175%, or 200%, so this reports inaccurate pixel sizes and macOS terminology. Show the actual physical display resolution instead.
- **Restore missing UI controls for features that already exist in the engine:**
  - **Manual zoom target mode:** `editor/mod.rs:1500-1533` already implements clicking/dragging a fixed zoom target on the preview canvas when `ZoomKind::Manual`, but `panels/zoom.rs` provides no control to switch a zoom region between `FollowCursor` and `Manual`. Add a segmented control (`Follow cursor` / `Fixed point`) to `panels/zoom.rs`.
  - **Preset padding bug:** `dialogs.rs:121` unconditionally resets `p.config.styles.padding_ratio = 0.02` at the end of `apply_preset()`, silently discarding the `paddingRatio` stored in user-saved presets.
  - **Export format selector:** `export_ui.rs:239` hardcodes `ex.format = Format::Mp4;` every frame, hiding WebM and GIF even though `export_ui.rs` and `crates/engine/src/gif.rs` implement them. Either expose a Format segmented control (`MP4` / `GIF` / `WebM`) or remove the dead code paths.
  - **Remember last export folder:** `export_ui.rs:333` reads `Prefs::load().export_dir`, but `launch()` never saves the chosen directory back to `Prefs`. Persist `prefs.export_dir = path.parent().map(Path::to_path_buf)` on export.
  - **Export completion actions:** Add an `"Open video"` button next to `"Show in folder"` and `"Done"` (`export_ui.rs:216-223`).
- **Add a Stop / Cancel button for caption generation (`panels/captions.rs:116-122`, `editor/actions.rs:211-253`, `whisper.rs`).**
  - Currently, clicking **"Generate captions"** disables the button to `"Working…"` with no way to stop a 500+ MB model download or a long CPU `whisper.cpp` transcription — and closing the editor leaves the `screencraft-whisper` thread burning up to 8 CPU cores in the background.
  - **How to wire it cleanly:**
    - Add `cancel: Arc<AtomicBool>` to `CapState` (`panels/captions.rs:50`) and pass a clone into `whisper::Request`.
    - While `busy` is true, replace the disabled `"Working…"` button (or place beside it) with a danger-styled **"Stop"** button (`kit::btn_line(ui, "Stop generating", true)`) that sets `cancel` and updates status to `"Stopping…"`. Also set `cancel` on editor close / `Drop`.
    - Check `cancel` in `whisper::download()` (between 256 KB reads, deleting the `.part` file on abort), in `whisper::speech_audio()` (per 1 s chunk), and inside `whisper::recognize()` via `params.set_abort_callback_safe(...)` so `whisper.cpp` aborts cleanly mid-transcription.
    - Handle a cancelled outcome cleanly in `take_caption_results()` (`actions.rs:228`) without showing the `"Caption generation failed"` error dialog.
- **Fix unlabeled / noisy HUD and inspector controls:**
  - **Recording HUD audio meters** (`recorder.rs:1001-1009`): Two 34×5 px meters are drawn with `let _ = tip;` (discarding their `"Microphone"` / `"System audio"` tooltips) and are shown even when an input is disabled. Add tiny mic/speaker icons or hover tooltips, and hide meters for disabled inputs.
  - **Timeline Speed dropdown** (`timeline.rs:153-171`): Always active and displays `1×` even when no clip is selected; picking a value silently does nothing unless `Sel::Slice` is selected. Disable it (with a tooltip `"Select a clip to change speed"`) or hide it when no clip is selected.
  - **Shadow angle slider** (`panels/appearance.rs:97`): Visible even when `Shadow style` is `Classic`, where `scene.rs:254` ignores `shadow.angle`. Hide or disable it unless `Directional` is selected.

### High-impact UX upgrades

- **Render the recorded audio waveform on the Video track.**
  `EditorState::open` (`editor/mod.rs:224-237`) already decodes the recording's audio peaks in a background thread and stores them in `self.waveform`, yet `timeline.rs` never draws `st.waveform`. Drawing the waveform inside Video track clips (just as music clips already draw their waveforms) makes finding pauses, verifying jump cuts, and aligning captions dramatically easier.
- **Improve Timeline ergonomics:**
  - **Pin the time ruler** at the top of the timeline viewport (`timeline.rs:504-516` currently offsets the ruler by `- v_off`, so scrolling vertically hides the ruler and playhead cap).
  - **Add a visible vertical scrollbar** when tracks overflow (`max_v > 0.0` at `timeline.rs:467`), especially since overlapping Notes dynamically expand into multiple lanes and can push Captions/Music below the fold with no visual indicator.
  - **Add magnetic snapping** (with `Alt` to bypass) when dragging or trimming blocks so edges snap cleanly to the playhead and adjacent cut/block boundaries.
  - **Live-preview edge frames while trimming** clips or zooms, instead of only moving a ghost rectangle until mouse release.
- **Make keyboard shortcuts discoverable (and the global hotkey visible):**
  - The editor has a great shortcut set (`Space` tap/hold 2×, `S` split, `X` jump cut, `Z` add zoom, `Q`/`E` start/end, `A`/`D` prev/next zoom, `←`/`→` frame / `Shift+←`/`→` 1 s, `F11` full preview), but most aren't surfaced anywhere. Add a compact **Keyboard Shortcuts** popover or section in the About modal, and include shortcuts in tooltips.
  - Show the global record/stop shortcut (`Ctrl+Shift+R`) in the Record and Stop button tooltips (`recorder.rs:797, 1016`), and allow changing or disabling it in Settings to avoid conflicts with browser hard-reload or GPU overlays.
- **Upgrade `kit::slider` interaction (`crates/app/src/kit.rs:69-104`):**
  - Paint an accent-filled progress bar from the left edge of the track to the thumb so current values read instantly at a glance.
  - Support **double-click to reset** to a default value and **click-to-type** on the right-aligned value label for exact numeric entry.
  - Add keyboard/accessibility support (arrow keys when focused) or wrap `egui::Slider` styling.
- **Direct on-canvas Crop handles:**
  - `panels/crop.rs` currently requires adjusting four percentage sliders (`Left`, `Top`, `Right`, `Bottom`). When `Tab::Crop` is active, render draggable edge/corner crop handles directly over the preview canvas (reusing the same coordinate mapping as `annotation_handles`).
- **Declutter the editor top bar (`editor/mod.rs:1217-1283`):**
  - Move the three external links (X, GitHub, Bug) out of the top-left corner into the About dialog (and reconcile the repo URL mismatch between `editor/mod.rs:1222` `MarshallNickolauson/ScreenCraft` and `dialogs.rs:565` `truevis/ScreenCraft`). This gives `Projects` and the editable project title the primary top-left position.
- **Recorder bar & Area picker polish:**
  - **Countdown visibility:** The 3-2-1 countdown currently renders only inside a 240×60 pill (`recorder.rs:964-975`). A brief, centered, click-through countdown badge on the target display (with `Esc to cancel`) gives clearer feedback before capture starts.
  - **Area picker multi-monitor & adjustment:** Allow switching displays from inside `area.rs` (or spawning the picker across the monitor under the cursor) and allow resizing/nudging a picked area before recording rather than forcing a full redraw.

---

## 2. Efficiency Improvements

### Rendering & GPU compositor (`crates/render/src/gpu.rs`, `crates/app/src/editor/mod.rs`)

1. **Stop re-rendering the GPU preview and CPU overlays on every idle repaint (biggest editor CPU/GPU win).**
   - `EditorState::show_preview` (`editor/mod.rs:1108-1120`) unconditionally calls `build_scene(...)` (which runs `tiny_skia` CPU rasterization and font shaping in `overlay::build`) and `self.compositor.render(&scene)` on *every* egui frame — including mouse hover, tooltips, or inspector scrolling when neither the playhead nor the project changed.
   - **Fix:** Maintain a lightweight render-cache key `(project_revision, frame_index, uploaded_key, preview_res, anim_time_bucket)` (only advancing `anim_time_bucket` when `playing` or an animated wallpaper is active). If the key matches the last rendered frame, skip `build_scene` and `compositor.render` and simply draw the existing registered `egui::TextureId`.
2. **Cache `wgpu::BindGroup`s in `Compositor` (`crates/render/src/gpu.rs:409-434, 508, 540, 566`).**
   - `Compositor::render` allocates 1–4 new `BindGroup` objects on every single frame (both in the editor and across thousands of export frames). Cache them and rebuild only when a slot/layer/target texture is recreated or cleared.
3. **Skip redundant full-frame CPU→GPU overlay uploads (`gpu.rs:304-327, 445-456`).**
   - `overlay::device_layer` caches its `Arc<Pixmap>`, yet `Compositor::render` still calls `upload_layer` (`queue.write_texture`) on every frame, re-uploading a full RGBA frame for the static device bezel. Store the last `Arc<Pixmap>` (or its pointer/generation) in `Layer` and skip `write_texture` when `Arc::ptr_eq` matches — just like `upload_bg` already does at `gpu.rs:330`.
4. **Avoid allocating a full-resolution `tiny_skia::Mask` when there are no content annotations (`crates/render/src/overlay.rs:203-209`).**
   - `annotations(..., "content", ...)` allocates a `w × h` `Mask` and rasterizes the rounded screen path on every frame even when `p.scene.annotations` has no active `"content"` entries at `g.t_src`. Early-out before allocating `Mask` if no annotation is active in that space.

### Export pipeline (`crates/engine/src/export.rs`, `crates/render/src/gpu.rs`)

5. **Double-buffer GPU readback (`gpu.rs:597-646`).**
   - `read_rgba()` submits `copy_texture_to_buffer`, calls `map_async`, and blocks on `device.poll(wait_indefinitely)` every frame. Pipelining 2–3 staging buffers (render frame `N` while reading back frame `N-1`) keeps the GPU and the `to_i420` conversion thread busy simultaneously.
6. **Move motion-blur sub-frame accumulation onto the GPU (`export.rs:354-378`).**
   - Motion blur is enabled by default in Export (`export_ui.rs:75`). During every zoom or pan, `render_frame` renders up to 6 sub-frames, performs **6 blocking GPU→CPU `read_rgba()` stalls**, rebuilds CPU overlays 6 times, and blends the RGBA bytes in a scalar CPU loop.
   - Accumulating sub-frames into an intermediate GPU texture (e.g. a tiny blend pass) and doing **one** `read_rgba()` per output frame cuts readback overhead by up to 6× during motion.

### Editor & UI-thread efficiency (`crates/app`)

7. **Eliminate per-frame disk I/O inside `egui` draw loops:**
   - `dialogs.rs:192-193, 226-227`: While the **Projects** popover is open, every repaint calls `Prefs::load()`, scans the recordings directory (`list_packages`), and opens/parses `project.json` for up to 8 projects.
   - `dialogs.rs:295`: While the **Presets** popover is open, every repaint reads and parses `presets.json` from disk.
   - `app.rs:313`: While recording in *"Record more"* mode, `draw_append` calls `Package::open(&target).load_project()` every frame just to format the project's name.
   - **Fix:** Load these once on open/transition and cache them in `Dialogs` / `App`.
8. **Stop cloning multi-megabyte base64 images on every undo step (`editor/mod.rs:404-412`, `appearance.rs:76, 103`).**
   - Background images and watermarks are stored as inline `data:image/...;base64,...` strings inside `Project`. `EditorState::edit_keyed` clones `self.project` up to 200 times into `self.undo`. A single 2 MB background image in `undo` can consume hundreds of MB of RAM and stall `project.json` saves.
   - **Fix:** Save picked images as files inside the `.screencraft` package folder (e.g., `background.png`, `watermark.png`) and reference them by relative filename in `Project` (keeping data-URL fallback for backwards compatibility).
9. **Don't restart audio on visual-only edits (`editor/mod.rs:387-395, 461-485`).**
   - `changed()` calls `restart_audio()` whenever `self.playing` is true — even if the user only dragged a shadow, padding, or background slider. `restart_audio()` calls `build_soundtrack()` synchronously on the UI thread (re-opening `screen.mp4` and spawning a new playback thread), causing audio glitches and UI hitches. Split edits into visual vs. audio-affecting (or debounce/offload soundtrack rebuilds).
10. **Flush `H264Decoder` instead of recreating it on every seek (`crates/engine/src/source.rs:144`).**
    - Every non-sequential seek (scrubbing backwards or jumping across keyframes) discards `self.decoder` and calls `H264Decoder::new(...)`, tearing down and re-instantiating the Media Foundation transform. Adding a `flush()` method (`MFT_MESSAGE_COMMAND_FLUSH`) makes timeline scrubbing noticeably snappier.
11. **Don't start a full screen capturer just for the idle system-audio meter (`crates/app/src/live.rs:197-229`).**
    - `Live::sync` spins up `create_capturer()` with `Target::Display(None)` at 5 fps and drops the video frames solely to meter system audio while idle. On Windows, WASAPI loopback capture in `screencraft-audio` can meter system audio directly without touching DXGI Desktop Duplication.
12. **Reduce per-frame allocations and idle wakeups in `kit.rs`, `timeline.rs`, and `recorder.rs`:**
    - `kit.rs:29-38` (`spaced_text`): Calls `painter.layout_no_wrap(ch.to_string(), ...)` once *per character* on every frame. Use `egui::text::LayoutJob` or cache galleys.
    - `kit.rs:17-26` (`fit_text`): Pops one char at a time with `format!("{s}…")` and `layout_no_wrap` in a loop every frame. Binary-search the char boundary or let `egui` truncate via `Galley` / `max_rows = 1`.
    - `timeline.rs:204-206`: Deep-clones `st.project.scene` and `slices` every frame solely to satisfy the borrow checker. Split block-building from `&mut EditorState` mutation to borrow directly.
    - `recorder.rs:306-318`: Polls `permissions()` every 2 s (a no-op on Windows) and spawns a `list_sources()` OS thread every 4 s even when idle with no menu open. Refresh sources lazily when opening Display/Window menus or on a longer interval.

---

## 3. Elegance & Maintainability

### Dead & leftover code to remove

- **`crates/app/src/editor/inspector.rs` (362 lines):** Completely unused (not even `mod inspector;` in `editor/mod.rs`) and references removed types (`Selection`, `Tab::Clip`, `Tab::Style`). Delete it.
- **`crates/app/src/theme.rs:172-209`:** Temporary compatibility layer (`Palette`, `palette()`, `section()`, `primary_button()`, `danger_button()`, `segmented()`) from the old library screen — unused anywhere in active code.
- **`crates/app/src/permissions.rs` & `App::show_permissions`:** macOS-only TCC permission flow; dead on Windows.
- **Minor unused fields/stubs:**
  - `EditorState::wants_back` (`editor/mod.rs:191`) — always `false`, return value of `EditorState::ui` is ignored.
  - `Recorder::finished` (`recorder.rs:225`) — never read or written.
  - `AudioState {}` (`panels/audio.rs:4-5`) — empty struct.
  - `fn finish(...) {}` (`timeline.rs:912`) — empty no-op function.

### Structural & architectural cleanup

- **Split the three 1,000+ line UI monoliths:**
  - `crates/app/src/editor/mod.rs` (1,692 lines) → extract `stage.rs` (preview, overlays, annotation/zoom/crop handles), `transport.rs`, `topbar.rs`, and `shortcuts.rs`.
  - `crates/app/src/recorder.rs` (1,240 lines) → extract `recorder/controller.rs`, `recorder/menus.rs`, and `recorder/hud.rs`.
  - `crates/app/src/editor/timeline.rs` (1,164 lines) → split the ~700-line `body()` function into `layout` (build blocks & geometry), `input` (hit-testing & drag state machine), and `paint` (blocks, waveform, playhead, scrollbar).
- **Extract shared UI chrome into `kit.rs` and `theme.rs`:**
  - **Modal helper (`kit::modal`):** `export_ui.rs:158-185`, `dialogs.rs:523-539`, and `dialogs.rs:585-599` each duplicate the dimmed backdrop `Area` + centered `Window` frame.
  - **Popover alignment:** `dialogs.rs:199-255` duplicates `popover()` for `projects_menu` only to left-align instead of right-align. Add an alignment parameter to `popover()`.
  - **Toggle widget:** `export_ui.rs:364-372` reimplements the 36×20 switch already drawn in `kit::toggle_row` (`kit.rs:120-124`). Extract `kit::toggle(ui, &mut bool)`.
  - **Semantic color tokens:** Move repeated raw literals (`Color32::from_rgb(0xb9, 0xc3, 0xd6)`, `0xcd, 0xd5, 0xe5`, `0x2d, 0x14, 0x05`, etc.) into named constants in `theme.rs`.
- **Replace stringly-typed discriminants with enums:**
  - `area.rs:181` passes `style: &'static str` (`"rec"`, `"hl"`, `"sel"`) instead of an enum.
  - `Project` fields like `Annotation.kind`, `Annotation.space`, `Background.kind`, `Background.motion`, `Transition.kind`, and `Styles.device` are raw `String`s matched with `.as_str()` across `core`, `render`, and `app`. Switching to `#[serde(rename_all = "kebab-case")]` enums removes string allocations in the render hot path and catches typos at compile time.
- **Pass `Prefs` explicitly instead of calling `Prefs::load()` ad hoc:**
  - `App` owns `self.prefs`, yet `export_ui.rs:333` and `dialogs.rs:192` re-read `settings.json` from disk directly. Passing the needed paths (or a shared `Prefs` handle) avoids redundant disk reads and makes saving `export_dir` straightforward.
- **Poison-safe locks in UI/worker communication:**
  - `app.rs`, `live.rs`, `playback.rs`, `export_ui.rs`, `compressor.rs`, and `overlay.rs` still use `std::sync::Mutex::lock().unwrap()`. Switching to `parking_lot::Mutex` (or a small helper) prevents a panicking worker thread from taking down the entire UI.

---

## Suggested Priority Order

1. **Quick UX/bug pass:** Fix mojibake (`area.rs`, `compressor.rs`), Windows shortcut labels + `Ctrl+Y`, Retina copy (`recorder.rs`), preset padding clobber (`dialogs.rs`), Manual Zoom toggle (`panels/zoom.rs`), `export_dir` persistence, and a Stop/Cancel button for caption generation (`captions.rs` + `whisper.rs`).
2. **Idle preview & disk-I/O caching:** Cache rendered preview frames in `EditorState::show_preview`, `Arc::ptr_eq` check in `Compositor::upload_layer`, skip empty `Mask` in `overlay.rs`, and remove per-frame `Prefs::load()` / `list_packages()` / `user_presets()` calls.
3. **Timeline UX upgrade:** Render the already-computed `EditorState.waveform` on the Video track, pin the ruler, add a vertical scrollbar, disable Speed when no clip is selected, and add playhead/edge snapping.
4. **Export & scrubbing performance:** Double-buffered GPU readback, GPU motion-blur accumulation, `H264Decoder::flush()` on seek, and storing background/watermark images as package files instead of inline base64.
5. **Codebase hygiene:** Delete `inspector.rs` / `permissions.rs` / legacy `theme.rs` shims, deduplicate modal/popover/toggle helpers, and split `editor/mod.rs`, `recorder.rs`, and `timeline.rs`.
