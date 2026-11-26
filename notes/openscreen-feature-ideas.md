# Feature Ideas from OpenScreen (`getopenscreen/openscreen`)

Reference: [github.com/getopenscreen/openscreen](https://github.com/getopenscreen/openscreen) (MIT-licensed screen recorder & editor, Electron + Rust/TypeScript).

ScreenCraft already covers much of OpenScreen's core feature set natively in Rust (auto-zoom, cursor smoothing/styles/click effects, 6-track timeline with ripple edits, on-device `whisper.cpp` captions + SRT, annotations, keystroke overlay, device frames, procedural wallpapers, music + ducking, motion blur, and MP4/GIF export).

Below are the genuinely useful features from OpenScreen that ScreenCraft does **not** yet have (or only partially has), ranked by value for Windows demo/tutorial creators.

---

## 1. High-Impact Additions

### 1.1 Transcript-based video editing
- **What OpenScreen does:** Displays the on-device Whisper transcription as an editable transcript; deleting words/phrases in the transcript automatically cuts the corresponding time ranges out of the video timeline.
- **Why it's great for ScreenCraft:** ScreenCraft already runs `whisper.cpp` locally and has slice splitting + ripple editing. Adding a transcript panel where you can select/delete filler words, stumbles, or pauses to create jump cuts is one of the fastest ways to edit spoken app demos.
- **Implementation fit:** Medium. Needs word-level timestamps from `whisper.cpp` (`token_timestamps`), a selectable transcript view in the Captions tab, and mapping deleted spans to timeline slice cuts.

### 1.2 Image annotations on the timeline
- **What OpenScreen does:** Supports dropping image overlays (logos, icons, callout graphics, secondary screenshots) onto the timeline as timed annotations alongside text and arrows.
- **Why it's great for ScreenCraft:** ScreenCraft currently has a global watermark, plus text/arrow/shape/blur/spotlight annotations, but no timed image callouts.
- **Implementation fit:** Small–Medium. Extend `AnnotationKind` with `Image { path, opacity, corner_radius }`, store picked images inside the `.screencraft` package folder, and composite them in `overlay.rs`.

### 1.3 Text & callout animation presets
- **What OpenScreen does:** Built-in entrance/exit animation presets for text annotations (e.g., fade, pop/scale, slide).
- **Why it's great for ScreenCraft:** Static annotations can feel abrupt next to ScreenCraft's smooth spring-physics zooms and cursor motion. A few simple easing presets (None / Fade / Pop / Slide) make callouts feel much more polished.
- **Implementation fit:** Small. Compute a `0.0..1.0` progress factor from `t_src` relative to the annotation's start/end in `overlay.rs` and apply alpha/offset/scale.

### 1.4 Timeline magnetic snapping with visual snap guides
- **What OpenScreen does:** Snaps dragged/trimmed blocks to the playhead and adjacent clip/zoom/annotation edges, drawing a vertical snap guide line when aligned.
- **Why it's great for ScreenCraft:** Aligning annotations, captions, and zoom blocks to cuts or audio peaks is tedious without snapping. (Also noted in [suggestions-gemini.md](suggestions-gemini.md).)
- **Implementation fit:** Small. Pure UI/timeline math in `crates/app/src/editor/timeline.rs` with `Alt` to temporarily bypass snapping.

---

## 2. Polish & Nice-to-Haves

### 2.1 Richer webcam PiP controls
- **What OpenScreen does:** 8 snap positions (corners + edges) plus drag-to-position, horizontal mirroring (flip), adjustable corner roundness/shape presets, and independent shadow control.
- **Why it's great for ScreenCraft:** Mirroring is especially expected by anyone recording a talking-head webcam, and 8-point snap + drag makes positioning effortless.
- **Implementation fit:** Small. Add `mirror: bool`, `roundness: f32`, and snap-point buttons to the Camera panel and compositor.

### 2.2 Subtitle translation
- **What OpenScreen does:** Optional subtitle translation directly from the generated captions.
- **Why it's great for ScreenCraft:** `whisper.cpp` already supports translating non-English speech to English offline (`set_translate(true)`), which is trivial to expose as a checkbox in the Captions panel. Translating *to* other languages could be an optional BYOK API feature later.
- **Implementation fit:** Trivial for Whisper's built-in translate-to-English; Small–Medium for arbitrary target languages via BYOK.

---

## 3. Power-User / Automation Ideas

### 3.1 Local MCP server (`screencraft-mcp`) for coding-agent integration
- **What OpenScreen does:** Exposes an MCP (Model Context Protocol) server so AI coding agents (Claude Code, Antigravity, Cursor, etc.) can trigger recordings, inspect projects, and apply edits programmatically.
- **Why it's great for ScreenCraft:**
  - **AI-assisted editing without bloating the GUI app:** Instead of adding a BYOK chat panel or API-key management to `ScreenCraft.exe`, MCP lets whatever agent you already use edit projects externally. The main app stays 100% local, lean, and free of AI/cloud UI.
  - **Automated PR & release-note demos:** After an agent finishes a feature, you can ask it to record a short demo of the target window, apply auto-zooms and captions, trim dead air, and export a 1080p MP4.
  - **Natural-language editing on real recordings:** Open your latest recording, cut silences longer than 2 s, speed up a build wait by 4×, blur a sensitive region for a time range, or add title callouts at topic changes based on the Whisper transcript.
  - **Batch processing & repurposing:** Loop over a folder of `.screencraft` packages to apply a preset, burn in captions, or re-export in multiple aspect ratios (16:9 for YouTube, 9:16 for X).
  - **Visual QA & bug-repro clips:** Record a failing window during automated testing, annotate the error frame, and export a compressed MP4/GIF for an issue tracker.
- **Why ScreenCraft's architecture fits MCP unusually well:**
  - **Structured project & input data:** `.screencraft` packages already store `project.json` and `input.json` (timestamps + coordinates of every click and keystroke), so an agent can place zooms and callouts accurately without guessing.
  - **Headless rendering & export already exist:** `screencraft-core`, `screencraft-engine`, and `screencraft-shot` are decoupled from `egui`. The MCP server can render single preview frames (PNG) for the agent to visually inspect framing/blurs/overlays and run full exports without opening a GUI window.
  - **Zero impact on `ScreenCraft.exe`:** Shipped as a separate workspace binary (`screencraft-mcp`) sharing `screencraft-core` and `screencraft-engine`.
- **Minimal v1 toolset sketch:**
  - *Projects:* `list_projects`, `get_project_summary` (duration, clips, zooms, captions, transcript, click/key events)
  - *Recording:* `start_recording`, `stop_recording` (target display/window/area, mic/system audio)
  - *Editing:* `trim_or_cut`, `set_clip_speed`, `add_zoom`, `add_annotation` (text/arrow/box/blur/spotlight), `apply_preset`, `generate_captions`
  - *Inspection & Export:* `render_frame(timestamp)` (returns PNG for visual verification), `export_video(format, resolution, fps, output_path)`
- **Implementation fit:** Medium. Separate binary crate (`crates/mcp` or a `[[bin]]` in `crates/app` / `crates/engine`) using stdio JSON-RPC.

*(Note: In-app BYOK AI chat editing is intentionally excluded from the regular `ScreenCraft.exe` app; `screencraft-mcp` covers agent-driven editing cleanly outside the GUI.)*

---

## Suggested Priority Order

1. **Quick wins:** Timeline snapping + guides, webcam mirror & snap positions, Whisper translate-to-English toggle, text animation presets.
2. **Medium efforts with high daily value:** Timed image annotations, transcript-based video editing.
3. **Bigger bet:** `screencraft-mcp` for external agent-driven demo recording, editing, and batch export.
