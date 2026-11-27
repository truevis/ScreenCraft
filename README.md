<p align="center">
  <img src="assets/icons/icon-1024.png" alt="ScreenCraft" width="128">
</p>

<h1 align="center">ScreenCraft (Windows)</h1>

<p align="center">
  <b>Screen recording and video editing for Windows 10 / 11; an open-source, from-scratch alternative to Screen Studio, built in Rust.</b><br>
  Automatic zooms that follow your cursor and typing, a camera bubble, captions, annotations,<br>
  device frames, music and sound effects, and MP4 / GIF export, in one native Rust app (no web view).
</p>

<p align="center">
  <img alt="Rust" src="https://img.shields.io/badge/core-Rust-b7410e?style=flat-square&logo=rust">
  <img alt="Windows 10 / 11" src="https://img.shields.io/badge/Windows%2010%20%2F%2011-native-0EA5C6?style=flat-square">
  <img alt="License: MIT" src="https://img.shields.io/badge/license-MIT-3a3a3a?style=flat-square">
  <img alt="Version 0.4.1" src="https://img.shields.io/badge/version-0.4.1-5b6ee1?style=flat-square">
  <img alt="Status: early alpha" src="https://img.shields.io/badge/status-early%20alpha-d69e2e?style=flat-square">
  <img alt="Pure Rust engines" src="https://img.shields.io/badge/engines-Rust%20%2B%20wgpu-2f9a72?style=flat-square">
</p>

<p align="center">
  <a href="#about-this-fork">About this fork</a> ·
  <a href="#screenshots">Screenshots</a> ·
  <a href="#what-it-does">Features</a> ·
  <a href="#install">Install</a> ·
  <a href="#using-it">Using it</a> ·
  <a href="#mcp-server">MCP Server</a> ·
  <a href="#shortcuts">Shortcuts</a> ·
  <a href="#how-it-is-built">How it is built</a> ·
  <a href="#development">Development</a> ·
  <a href="#license">License</a>
</p>

<br>

> **Origin.** This repository ([truevis/ScreenCraft](https://github.com/truevis/ScreenCraft), branch
> [`ci/windows-installer`](https://github.com/truevis/ScreenCraft/tree/ci/windows-installer)) is a
> Windows-focused fork of [MarshallNickolauson/ScreenCraft](https://github.com/MarshallNickolauson/ScreenCraft)
> by Marshall Nickolauson. For other platforms, see the original repository.
>
> **Not affiliated.** ScreenCraft is an independent, open project that is
> *inspired by* the idea behind [Screen Studio](https://screen.studio). It is not
> affiliated with, endorsed by, or connected to Screen Studio or its makers in any
> way. "Screen Studio" is a trademark of its respective owner. ScreenCraft contains no
> code, artwork or other assets from that product.

## About this fork

This fork focuses on making ScreenCraft solid on **Windows 10 / 11**, including laptops with hybrid
Intel + NVIDIA graphics and high-DPI displays.

### What's new in this fork

**Fixes**
- Stopping a recording no longer gives a blank video and an error.
- No console window behind the app.
- The editor window fits on the screen it opens on (including 200% scaling).
- The recorder bar starts centred and remembers its position across monitors.
- No white or black overlay covering the screen while recording on hybrid Intel / NVIDIA laptops.
- The recording border is a 2-pixel hairline on the very edges of the recorded screen, kept out of the video.
- Microphone audio no longer crackles (capture timing jitter caused inserted silence and dropped samples).
- Recording folders are named in local time, not UTC.
- *Show in folder* after export opens the right folder.
- Timeline audio waveform combines both system audio and microphone tracks (so microphone audio is visible even when system audio is silent).

**Features**
- **Webcam-only recording (`Camera`):** record straight from a webcam (16:9 or `9:16` vertical) with a live draggable floating viewfinder above the bar, microphone capture, and automatic pause jump-cuts on open.
- **Projects menu:** New recording, Open project…, **Record more for this project**, and **Add a
  recording or video to this project** (existing edits kept).
- **Timeline video management & instant multi-segment append:** `+ Add video` and `Record` buttons
  in the timeline toolbar and a right-click clip context menu (insert/record before or after,
  duplicate, delete). Footage is added as instant `Media.segments` without full re-encoding, and
  auto-zooms are generated for newly added recordings.
- **Arrow-key nudging:** `←` / `→` nudge selected captions, zooms, camera layouts, annotations, and
  music clips on the timeline.
- **Native export resolution:** Export defaults to the edited video's resolution, with a **Native**
  option for non-standard dimensions.
- **Discard** button in the editor top bar with a confirmation dialog to permanently delete the open
  recording and its saved project folder.
- **Save** button, and a **full-screen preview** (`F11`, `Esc` to leave).
- Music **ripples** when a clip is trimmed or deleted.
- Defaults tuned for **professional app demos**: edge-to-edge video on black, subtitle-style
  captions, click ripple, 1.6× auto-zoom, on-screen shortcuts, music ducked under speech.
- **Transcript-based video editing:** clickable word-level transcript in the Captions tab with
  `Shift`-click range selection, 1-click filler word removal, clickable pause pills, `Del` to
  create non-destructive jump cuts, and click-to-restore for struck-through words.
- **Offline subtitle translation:** **Translate to English** toggle in the Captions panel powered
  by multilingual `whisper.cpp` models.
- **Timed image annotations & animation presets:** drop logos/callouts onto the Notes track
  (stored in the `.screencraft` package) with opacity, corner radius, `Shift`-resize aspect lock,
  and `None` / `Fade` / `Pop` / `Slide` entrance/exit presets with adjustable speed.
- **Timeline magnetic snapping:** toolbar **Snap** toggle (`Alt` to temporarily bypass) with an
  amber snap guide line and ruler timestamp badge.
- **Richer webcam PiP controls:** drag and corner-resize the camera bubble directly on the preview
  with 8-point edge/corner snapping (`Alt` for free placement), `Circle` shape preset, horizontal
  mirror, and independent shadow strength.
- **Local MCP server (`screencraft-mcp`):** standalone stdio JSON-RPC server installed beside
  `ScreenCraft.exe` so coding agents can record, inspect transcripts, edit timelines, preview
  frames, and export videos without bloating the GUI app.
- **Loudness target & background noise reduction:** one-click loudness normalization in the Audio
  tab (YouTube `-14 LUFS`, Podcast `-16 LUFS`, Broadcast `-23 LUFS`, or custom target with a
  `-1.0 dBTP` true-peak limiter) and spectral noise reduction for steady fan noise, hum, and hiss.
- The built-in update check is removed.

**Build tooling**
- `scripts/build-windows.ps1` builds the app, packs the NSIS installer, and copies the executable to
  `%LOCALAPPDATA%\Programs\ScreenCraft\ScreenCraft.exe`, with `sccache` support and a `-Fast` mode
  (about 20 s for a one-file change).
- A GitHub Actions workflow (`build-windows.yml`) builds the same installer and runs the tests.

## Screenshots

The editor with the six-track timeline (video, zoom, camera, notes, captions, music):

![The editor](docs/screenshots/editor.jpg)

The recorder bar:

![Recorder bar](docs/screenshots/recorder.jpg)

## What it does

**Recording**
- A floating bar at the bottom of the screen: **Display / Window / Area / Camera** (drag out an area on the desktop, or record webcam-only with a live floating viewfinder and a `9:16` portrait toggle), camera, microphone and system audio with level meters, resolution and frame rate, and a 3-2-1 countdown. While recording it shrinks to a timer, meters, discard and Stop, and a thin red border (kept out of the video) marks the area being recorded.
- System audio (WASAPI loopback) and the microphone as separate, time-aligned tracks; the camera is
  recorded to its own file.
- The pointer is not baked into the picture. Cursor motion, clicks and shortcuts are logged on the
  same clock so the editor can redraw, smooth and restyle them.
- Global shortcut: **Ctrl+Shift+R**.

**Projects** (the **Projects** menu in the editor, the folder button on the bar)
- **New recording**, **Open project…** (any earlier recording), and the recent projects.
- **Record more for this project** and **Add a recording or video to this project…**: append footage
  or insert it before/after any clip via the **Projects** menu, the timeline toolbar (`+ Add video`,
  `Record`), or a clip's right-click context menu, using instant multi-segment copying without full
  re-encoding; zooms, captions and other edits stay where they are (and auto-zooms are generated
  for newly added recordings).

**Editing** (one window per project; a **Save** button, `Ctrl+S`, and unsaved edits are asked about before it closes)
- **Timeline:** video, zoom, camera, notes (annotations), captions and music tracks. Trim, split,
  reorder, duplicate, per-clip speed, idle fast-forward, speed ramps, freeze frames, and **magnetic
  snapping** (`Alt` to bypass) with visual snap guides. Edits ripple: zooms, captions and notes
  follow the footage, and music after a trimmed or deleted clip moves with it. Zooms are generated
  automatically the first time a recording opens.
- **Full-screen preview** (`F11`, or the button beside the preview quality; `Esc` leaves).
- **Zoom and pan:** automatic regions from clicks and typing, or your own; follow the cursor or hold a point.
- **Cursor and motion:** size, four styles, smoothing, click effects, highlight, camera smoothness.
- **Appearance:** twelve procedural wallpapers (still or drifting / bokeh / waves / dust), gradient,
  colour or your own image, optional blur, padding, corners, shadow, watermark, and **device frames**
  (laptop, monitor, tablet, phone, browser).
- **Camera:** a bubble (`Circle`, `Square`, `4:3`, `16:9`, `3:4`) or side-by-side / large /
  camera-only layouts, dragged and resized directly on the preview with 8-point snapping, horizontal
  mirror, independent shadow strength, and per-block overrides over time.
- **Annotate:** text, arrows, boxes, circles, highlights, timed **images**, blur, pixelate and
  spotlight, dragged on the preview with `None` / `Fade` / `Pop` / `Slide` animations; on-screen
  **shortcut display**.
- **Captions & transcript editing:** on-device speech recognition and **Translate to English** with
  `whisper.cpp` (the model is downloaded once, audio never leaves the machine), word-level
  **transcript editing** (cut words, filler words, or pause gaps straight from the transcript, or
  restore them any time), SRT import / export, and Subtitle / Minimal / TikTok styles.
- **Transitions** at cuts: dissolve, fade, flash, blur, slide, zoom.
- **Audio:** levels, **loudness target** normalization (YouTube `-14 LUFS`, Podcast `-16 LUFS`,
  Broadcast `-23 LUFS` with `-1.0 dBTP` ceiling limiter), **background noise reduction** for fans,
  hum, and hiss, **music** clips with fades and ducking under speech, and click / typing **sound
  effects**.
- **Crop, aspect ratio, presets, undo / redo and raw-file access.**

**Defaults for app demos.** A new project fills the frame edge to edge on solid black (padding,
corners and wallpapers are opt-in), with sentence-case subtitles on a dark box, a click ripple,
1.6× auto-zoom, on-screen shortcuts, and music at 30% ducked under speech.

**Export**
| Format | Video | Audio |
| --- | --- | --- |
| MP4 | H.264 through Windows Media Foundation, or AV1 (rav1e) | Opus |
| WebM | AV1 (and VP9 when built with libvpx) | Opus |
| GIF | 15 fps, up to 540 px | none |

Native (defaults to the edited video's resolution) or 720p to 4K at 30 / 60 fps, four quality levels, optional motion blur and an optional **compress after
export** pass. A standalone **compressor** window re-encodes any H.264 MP4 smaller. Exports of the
software codecs are deterministic. ScreenCraft bundles **no H.264 codec**; see [COMPLIANCE.md](COMPLIANCE.md).

## Install

Requires **Windows 10 / 11** (64-bit). Capture uses DXGI Desktop Duplication and WASAPI loopback for
system audio.

Build from source:
1. Install [Rust](https://rustup.rs) (MSVC toolchain) and the Visual Studio Build Tools with the
   *Desktop development with C++* workload, then install the remaining build dependencies:
   ```powershell
   winget install Kitware.CMake LLVM.LLVM NSIS.NSIS Mozilla.sccache
   ```
2. Clone and build:
   ```powershell
   git clone -b ci/windows-installer https://github.com/truevis/ScreenCraft.git
   cd ScreenCraft
   powershell -ExecutionPolicy Bypass -File scripts\build-windows.ps1
   ```
   Add `-Fast` for quick test builds (no LTO, incremental: a one-file change rebuilds in about 20 s).
   `sccache` is used automatically when it is installed.
3. Run the `screencraft_<version>.exe` installer (e.g. `screencraft_0_4_1.exe`) created in the
   repository folder. It installs per user to `%LOCALAPPDATA%\Programs\ScreenCraft` (pass `/S` for
   a silent install).

## Using it

1. Pick **Display / Window / Area / Camera** (`9:16` toggle for vertical areas or talking-head webcams), choose the camera and audio inputs, and press **Record** (or **Ctrl+Shift+R**). Press **Stop** in the bar, or press **Ctrl+Shift+R** again.
2. The recording opens in the editor with zooms generated for screen recordings or pause jump-cuts generated for webcam-only recordings.
3. Adjust the look, trim and split, add annotations, captions and music, then **Export**.

**Locations**
- Recordings: `%USERPROFILE%\Videos\ScreenCraft` as `Recording <date> <time>.screencraft` folders
  (local time); the location can be changed from the **Projects** menu.
- Settings: `%APPDATA%\ScreenCraft\settings.json`
- Log: `%LOCALAPPDATA%\ScreenCraft\screencraft.log`

## MCP server

`screencraft-mcp.exe` is installed next to `ScreenCraft.exe` in `%LOCALAPPDATA%\Programs\ScreenCraft`
and speaks MCP (Model Context Protocol) over stdio JSON-RPC so coding agents (Antigravity, Claude
Code, Cursor, etc.) can record demos, inspect transcripts and click logs, edit projects, render PNG
frames for visual verification, and export MP4 / WebM / GIF files headlessly.

Example client configuration:

```json
{
  "mcpServers": {
    "screencraft": {
      "command": "%LOCALAPPDATA%\\Programs\\ScreenCraft\\screencraft-mcp.exe"
    }
  }
}
```

Exposed tools:
- **Projects:** `list_projects`, `get_project_summary` (timeline, transcript, click & shortcut events)
- **Recording:** `list_capture_sources`, `start_recording` (`display` / `window` / `area` / webcam-only `camera` with `vertical`), `stop_recording`
- **Editing:** `trim_or_cut`, `set_clip_speed`, `add_zoom`, `add_annotation`, `apply_preset`, `generate_captions`
- **Inspection & Export:** `render_frame` (returns base64 PNG), `export_video`

### Shortcuts

`Space` play / pause (hold for 2×) · `S` split · `Z` add a zoom · `Delete` / `Backspace` remove the selection ·
`Ctrl+D` duplicate · `Q` / `E` start / end · `A` / `D` previous / next zoom · `←` `→` step a frame (or nudge the selected timeline item) ·
`Ctrl+Z` / `Ctrl+Shift+Z` (or `Ctrl+Y`) undo / redo · `Ctrl+S` save · `Ctrl+E` export ·
`Ctrl+A` select all zooms · `F11` full-screen preview (`Esc` to leave) ·
`Ctrl + scroll` zoom the timeline · `Ctrl+Shift+R` start / stop recording

## How it is built

| Crate | What it does |
| --- | --- |
| `screencraft-core` (`crates/core`) | Pure Rust: project model, spring physics, timeline mapping, auto-zoom generation, per-frame camera / cursor render track. |
| `screencraft-capture` | DXGI Desktop Duplication screen capture, constant-frame-rate resampler, cursor / click / key logging. |
| `screencraft-audio` | WASAPI (via `cpal`) microphone and loopback streams on a shared clock, the pure `StreamAligner` that turns timestamped blocks into gap-free 48 kHz stereo, spectral noise reduction, and LUFS loudness normalization / true-peak limiting. |
| `screencraft-encode` | Sans-I/O: BGRA to I420, AV1 / VP9 and Opus encoders, Windows Media Foundation H.264, MP4 and WebM muxers that emit `WriteAt` operations, an MP4 demuxer (including fragmented MP4) and the `Exporter` state machine. |
| `screencraft-render` | The wgpu compositor (WGSL), the CPU-side scene builder and the tiny-skia overlays (device frames, annotations, captions). Shared by the preview and the exporter. |
| `screencraft-engine` | The recording session (screen, audio, camera), project packages, frame / audio sources, the soundtrack mixer, and the export, GIF and compressor jobs. |
| `screencraft` (`crates/app`) | The egui application, plus `screencraft-shot`, a headless screenshot tool. |
| `screencraft-mcp` (`crates/mcp`) | Standalone local stdio MCP server for agent-driven recording, editing, frame inspection, and export. |

Things worth knowing:

- **One timeline.** Every stream is stamped on a single monotonic clock; time zero is the first
  video frame, and the audio aligners pad or trim to it.
- **Sans-I/O muxing.** `Exporter` takes frames and PCM and returns `WriteAt { offset, data }`
  operations. Files, memory buffers and threads belong to the caller (`FileSink`, `MemorySink`,
  `ThreadedExporter`), which is what makes the output reproducible and easy to test.
- **No web view.** The whole UI is egui; a worker thread decodes the recording and the compositor
  draws exactly what the exporter will.
- **No bundled H.264.** Encoding and decoding use Windows Media Foundation's built-in codec.
- **Wallpapers and cursors** are generated procedurally; no third-party artwork is bundled.

A `.screencraft` package contains `screen.mp4` (H.264 video plus one Opus track per audio source),
`camera.mp4` (when the camera was on), `input.json` (cursor, clicks, keys), `project.json` and any
music or annotation/background image files you added.

## Development

```powershell
cargo test --workspace --release        # unit and end-to-end tests (needs a GPU adapter for render tests)
cargo test --release -p screencraft-encode --test roundtrip h264_mp4_roundtrip_decodes
cargo test --release -p screencraft-engine --test record_synthetic
cargo run --release -p screencraft      # run the app
cargo run --release -p screencraft-engine --example make_demo -- "$env:TEMP\demo"     # synthetic recording
cargo run --release -p screencraft --bin screencraft-shot -- out.png --open "$env:TEMP\demo\Demo.screencraft" --dark
cargo run --release -p screencraft-engine --example export -- "$env:TEMP\demo\Demo.screencraft" out.mp4 h264 1080 30
cargo run --release -p screencraft-audio --example mic_probe -- "$env:TEMP\mic" 6     # microphone capture check
python scripts\gen-notices.py           # refresh THIRD-PARTY-NOTICES.md after dependency changes
```

Environment variables for testing: `SCREENCRAFT_RECORDINGS_DIR` (library folder),
`SCREENCRAFT_OPEN` (open a package at start), `SCREENCRAFT_AUTORECORD=<seconds>` (record the main
display, report, quit).

## Credits

AI-assisted coding by Claude, Cursor, and Antigravity.

## License

This project is licensed under the MIT License. By using this software, you agree that the authors are not liable for any issues, damages, or claims arising from its use.

See [LICENSE](LICENSE) for the full text, [THIRD-PARTY-NOTICES.md](THIRD-PARTY-NOTICES.md) for the open-source
components ScreenCraft uses, and [COMPLIANCE.md](COMPLIANCE.md) for the legal and compliance guide.

## Known limitations

- Overlay camera bubble video from an added recording is not carried over.
- HEVC export / compression is not built; the compressor reads H.264 MP4s and carries Opus audio only.
- Large sources (5K+) decode slower than real time when scrubbing.
- The installer is unsigned, so Windows SmartScreen may warn on first launch.
