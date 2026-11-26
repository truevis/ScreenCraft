<p align="center">
  <img src="assets/icons/icon-1024.png" alt="ScreenCraft" width="128">
</p>

<h1 align="center">ScreenCraft</h1>

<p align="center">
  <b>Screen recording and video editing; an open-source, from-scratch alternative to Screen Studio, built in Rust.</b><br>
  Automatic zooms that follow your cursor and typing, a camera bubble, captions, annotations,<br>
  device frames, music and sound effects, and MP4 / GIF export, in one native Rust app (no web view).
</p>

<p align="center">
  <img alt="Rust" src="https://img.shields.io/badge/core-Rust-b7410e?style=flat-square&logo=rust">
  <img alt="macOS · Windows · Linux" src="https://img.shields.io/badge/macOS%20%C2%B7%20Windows%20%C2%B7%20Linux-native-0EA5C6?style=flat-square">
  <img alt="License: MIT" src="https://img.shields.io/badge/license-MIT-3a3a3a?style=flat-square">
  <img alt="Version 0.4.1" src="https://img.shields.io/badge/version-0.4.1-5b6ee1?style=flat-square">
  <img alt="Status: early alpha" src="https://img.shields.io/badge/status-early%20alpha-d69e2e?style=flat-square">
  <img alt="Pure Rust engines" src="https://img.shields.io/badge/engines-Rust%20%2B%20wgpu-2f9a72?style=flat-square">
</p>

<p align="center">
  <a href="#windows-edition-this-fork"><b>Windows edition</b></a> ·
  <a href="#screenshots">Screenshots</a> ·
  <a href="#what-it-does">Features</a> ·
  <a href="#install">Install</a> ·
  <a href="#using-it">Using it</a> ·
  <a href="#shortcuts">Shortcuts</a> ·
  <a href="#how-it-is-built">How it is built</a> ·
  <a href="#development">Development</a> ·
  <a href="#license">License</a>
</p>

<br>

> **Not affiliated.** ScreenCraft is an independent, open project that is
> *inspired by* the idea behind [Screen Studio](https://screen.studio). It is not
> affiliated with, endorsed by, or connected to Screen Studio or its makers in any
> way. "Screen Studio" is a trademark of its respective owner. ScreenCraft contains no
> code, artwork or other assets from that product.

## Windows edition (this fork)

This fork ([truevis/ScreenCraft](https://github.com/truevis/ScreenCraft), branch
[`ci/windows-installer`](https://github.com/truevis/ScreenCraft/tree/ci/windows-installer)) focuses on
making ScreenCraft solid on **Windows 10 / 11**, including laptops with Intel + NVIDIA graphics and
high-DPI screens. Original project: [MarshallNickolauson/ScreenCraft](https://github.com/MarshallNickolauson/ScreenCraft).

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
- **Projects menu:** New recording, Open project…, **Record more for this project**, and **Add a
  recording or video to this project** (joined onto the end, existing edits kept).
- **Discard** button in the editor top bar with a confirmation dialog to permanently delete the open
  recording and its saved project folder.
- **Save** button, and a **full-screen preview** (`F11`, `Esc` to leave).
- Music **ripples** when a clip is trimmed or deleted.
- Defaults tuned for **professional app demos**: edge-to-edge video on black, subtitle-style
  captions, click ripple, 1.6× auto-zoom, on-screen shortcuts, music ducked under speech.
- The built-in update check is removed.

**Build tooling**
- `scripts/build-windows.ps1` builds the app, packs the installer, and copies the executable to
  `%LOCALAPPDATA%\Programs\ScreenCraft\ScreenCraft.exe`, with sccache and a `-Fast` mode (about
  20 s for a one-file change).
- A GitHub Actions workflow (`build-windows.yml`) builds the same installer and runs the tests.

### Install the Windows edition

Build it yourself:
1. Install [Rust](https://rustup.rs) (MSVC toolchain) and the Visual Studio Build Tools with
   *Desktop development with C++*, then:
   ```powershell
   winget install Kitware.CMake LLVM.LLVM NSIS.NSIS Mozilla.sccache
   ```
2. Clone and build:
   ```powershell
   git clone -b ci/windows-installer https://github.com/truevis/ScreenCraft.git
   cd ScreenCraft
   powershell -ExecutionPolicy Bypass -File scripts\build-windows.ps1
   ```
3. Run the `screencraft_<version>.exe` installer it creates in the repository folder.

Recordings are saved in `Videos\ScreenCraft`, settings in `%APPDATA%\ScreenCraft`, and a log in
`%LOCALAPPDATA%\ScreenCraft\screencraft.log`. Start and stop recording with **Ctrl+Shift+R**.

**Tip:** for clean audio, turn off *Audio enhancements* for the microphone in Windows Sound settings,
and avoid recording through a Bluetooth headset's microphone (it switches the headset to
low-quality hands-free audio).

## Screenshots

The editor with a generated demo recording (a synthetic "app", not real footage): the Annotate panel,
and the six-track timeline (video, zoom, camera, notes, captions, music).

![The editor](docs/screenshots/editor.png)

Paused inside a zoom region; the cursor and click ripple are drawn by the same shader the exporter uses.

![Zoomed in the editor](docs/screenshots/editor-zoom.png)

Cursor and camera-motion settings:

![Cursor settings](docs/screenshots/editor-look.png)

The recorder bar:

![Recorder bar](docs/screenshots/recorder.png)

## What it does

**Recording**
- A floating bar at the bottom of the screen: **Display / Window / Area** (drag out the area on the
  desktop), camera, microphone and system audio with level meters, resolution and frame rate, and a
  3-2-1 countdown. While recording it shrinks to a timer, meters, discard and Stop, and a thin red
  border (kept out of the video) marks the area being recorded.
- System audio and the microphone as separate, time-aligned tracks; the camera is recorded to its own file.
- The pointer is not baked into the picture. Cursor motion, clicks and shortcuts are logged on the
  same clock so the editor can redraw, smooth and restyle them.
- Global shortcut: **⌘⇧R** on macOS, **Ctrl+Shift+R** elsewhere.

**Projects** (the **Projects** menu in the editor, the folder button on the bar)
- **New recording**, **Open project…** (any earlier recording), and the recent projects.
- **Record more for this project** and **Add a recording or video to this project…**: the new
  footage is joined onto the end as its own clip; zooms, captions and other edits stay where they are.

**Editing** (one window per project; a **Save** button, `Ctrl+S`, and unsaved edits are asked about before it closes)
- **Timeline:** video, zoom, camera, notes (annotations), captions and music tracks. Trim, split,
  reorder, duplicate, per-clip speed, idle fast-forward, speed ramps and freeze frames. Edits ripple:
  zooms, captions and notes follow the footage, and music after a trimmed or deleted clip moves
  with it. Zooms are generated automatically the first time a recording opens.
- **Full-screen preview** (`F11`, or the button beside the preview quality; `Esc` leaves).
- **Zoom and pan:** automatic regions from clicks and typing, or your own; follow the cursor or hold a point.
- **Cursor and motion:** size, four styles, smoothing, click effects, highlight, camera smoothness.
- **Appearance:** twelve procedural wallpapers (still or drifting / bokeh / waves / dust), gradient,
  colour or your own image, optional blur, padding, corners, shadow, watermark, and **device frames**
  (laptop, monitor, tablet, phone, browser).
- **Camera:** a bubble or side-by-side / large / camera-only layouts, changing over time in blocks.
- **Annotate:** text, arrows, boxes, circles, highlights, blur, pixelate and spotlight, dragged on
  the preview; on-screen **shortcut display**.
- **Captions:** on-device speech recognition with whisper.cpp (the model is downloaded once, audio
  never leaves the machine), SRT import / export, Subtitle / Minimal / TikTok styles.
- **Transitions** at cuts: dissolve, fade, flash, blur, slide, zoom.
- **Audio:** levels, **music** clips with fades and ducking under speech, and click / typing **sound effects**.
- **Crop, aspect ratio, presets, undo / redo and raw-file access.**

**Defaults for app demos.** A new project fills the frame edge to edge on solid black (padding,
corners and wallpapers are opt-in), with sentence-case subtitles on a dark box, a click ripple,
1.6× auto-zoom, on-screen shortcuts, and music at 30% ducked under speech.

**Export**
| Format | Video | Audio |
| --- | --- | --- |
| MP4 | H.264 through the operating system's own encoder (VideoToolbox on macOS, Media Foundation on Windows), or AV1 (rav1e) | Opus |
| WebM | AV1 (and VP9 when built with libvpx) | Opus |
| GIF | 15 fps, up to 540 px | none |

720p to 4K at 30 / 60 fps, four quality levels, optional motion blur and an optional **compress after
export** pass. A standalone **compressor** window re-encodes any H.264 MP4 smaller. Exports of the
software codecs are deterministic. ScreenCraft bundles **no H.264 codec**; see [COMPLIANCE.md](COMPLIANCE.md).

## Install

### macOS

Build from source (see [Build from source (macOS)](#build-from-source-macos) below). The app is ad-hoc
signed when bundled locally (not notarised), so macOS may block it the first time:
**System Settings → Privacy & Security → Open Anyway**, or
`xattr -dr com.apple.quarantine /Applications/ScreenCraft.app`.

Requires macOS 15 (Sequoia) or newer; developed on Apple silicon.

| Permission | Why |
| --- | --- |
| Screen Recording | capturing the display, windows and areas (required) |
| Input Monitoring | noticing shortcuts and typing for auto-zoom (optional) |
| Accessibility | following the text caret while typing (optional) |
| Microphone / Camera | only when you turn them on |

macOS applies Screen Recording after a restart. The bundle is signed with a stable identity, so
permissions survive rebuilds.

### Windows

For this fork's Windows-focused build, see [Install the Windows edition](#install-the-windows-edition)
above. Capture uses DXGI Desktop Duplication and WASAPI loopback for system audio. The installer is
unsigned, so SmartScreen may warn. On laptops with two graphics chips, the NVIDIA overlay may
announce itself when ScreenCraft starts; it isn't used and can be switched off in the NVIDIA app.

Build with Rust (MSVC toolchain, *Desktop development with C++*), CMake, LLVM and NSIS;
the installer is named after the version (`screencraft_0_4_1.exe`):

```powershell
powershell -ExecutionPolicy Bypass -File scripts\build-windows.ps1
```

Add `-Fast` for quick test builds (no LTO, incremental: a one-file change rebuilds in about 20 s).
`sccache` is used automatically when it is installed.

### Linux

Build from source (needs `xdg-desktop-portal` with a ScreenCast backend and PipeWire at runtime):

```bash
sudo apt install build-essential cmake pkg-config libpipewire-0.3-dev libasound2-dev \
     libclang-dev libx11-dev libxtst-dev libxi-dev libwayland-dev libxkbcommon-dev
cargo build --release -p screencraft
./target/release/screencraft
```

The portal's own dialog picks the screen or window. Pointer position comes from the stream's cursor
metadata; clicks and key shortcuts are only available on X11 (Wayland has no global input hook for
normal applications), so click-based auto-zoom needs an X11 session.

### Build from source (macOS)

```bash
git clone https://github.com/MarshallNickolauson/ScreenCraft.git
cd ScreenCraft
./scripts/bundle.sh --install     # builds a release app and copies it to ~/Applications
open ~/Applications/ScreenCraft.app
```

Needs [Rust](https://rustup.rs), CMake and the Xcode command line tools. If `libvpx` is installed
(`brew install libvpx`) the bundle is built with VP9 export. `./scripts/dmg.sh` also builds the
installer, `screencraft_0_4_1.dmg` (named after the version, like the Windows installer).

## Using it

1. Pick **Display / Window / Area**, choose the camera and audio inputs, press **Record** (or the
   shortcut). Press **Stop** in the bar, or use the shortcut again.
2. The recording opens in the editor with zooms already generated from your clicks and typing.
3. Adjust the look, trim and split, add annotations, captions and music, then **Export**.

Recordings live in `~/Movies/ScreenCraft` (`Videos\ScreenCraft` on Windows and Linux) as
`Recording <date> <time>.screencraft` folders (local time); the location can be changed from the **Projects** menu. 0.3 recordings
open as they are.

### Shortcuts

`Space` play / pause (hold for 2×) · `S` split · `Z` add a zoom · `Delete` remove the selection ·
`⌘D` duplicate · `Q` / `E` start / end · `A` / `D` previous / next zoom · `←` `→` step a frame ·
`⌘Z` / `⇧⌘Z` undo / redo · `⌘S` save · `⌘E` export · `⌘A` select all zooms ·
`F11` full-screen preview · `Ctrl/⌘ + scroll` zoom the timeline · `⌘⇧R` / `Ctrl+Shift+R` start / stop recording

## How it is built

| Crate | What it does |
| --- | --- |
| `screencraft-core` (`crates/core`) | Pure Rust: project model, spring physics, timeline mapping, auto-zoom generation, per-frame camera / cursor render track. |
| `screencraft-capture` | One `Capturer` per platform (ScreenCaptureKit, DXGI Desktop Duplication, PipeWire + portal), constant-frame-rate resampler, cursor / click / key logging. |
| `screencraft-audio` | cpal microphone and loopback streams on a shared clock, and the pure `StreamAligner` that turns timestamped blocks into gap-free 48 kHz stereo. |
| `screencraft-encode` | Sans-I/O: BGRA to I420, AV1 / VP9 and Opus encoders, the operating system's H.264 (VideoToolbox / Media Foundation), MP4 and WebM muxers that emit `WriteAt` operations, an MP4 demuxer (including fragmented MP4) and the `Exporter` state machine. |
| `screencraft-render` | The wgpu compositor (WGSL), the CPU-side scene builder and the tiny-skia overlays (device frames, annotations, captions). Shared by the preview and the exporter. |
| `screencraft-engine` | The recording session (screen, audio, camera), project packages, frame / audio sources, the soundtrack mixer, and the export, GIF and compressor jobs. |
| `screencraft` (`crates/app`) | The egui application, plus `screencraft-shot`, a headless screenshot tool. |

Things worth knowing:

- **One timeline.** Every stream is stamped on a single monotonic clock; time zero is the first
  video frame, and the audio aligners pad or trim to it.
- **Sans-I/O muxing.** `Exporter` takes frames and PCM and returns `WriteAt { offset, data }`
  operations. Files, memory buffers and threads belong to the caller (`FileSink`, `MemorySink`,
  `ThreadedExporter`), which is what makes the output reproducible and easy to test.
- **No web view.** The whole UI is egui; a worker thread decodes the recording and the compositor
  draws exactly what the exporter will.
- **No bundled H.264.** Encoding and decoding use the operating system's licensed codec.
- **Wallpapers and cursors** are generated procedurally; no third-party artwork is bundled.

A `.screencraft` package contains `screen.mp4` (H.264 video plus one Opus track per audio source),
`camera.mp4` (when the camera was on), `input.json` (cursor, clicks, keys), `project.json` and any
music files you added.

## Development

```bash
cargo test --workspace --release        # unit and end-to-end tests (needs a GPU adapter for render tests)
cargo run --release -p screencraft      # run the app
cargo run --release -p screencraft-engine --example make_demo -- /tmp/demo     # synthetic recording
cargo run --release -p screencraft --bin screencraft-shot -- out.png --open /tmp/demo/Demo.screencraft --dark
cargo run --release -p screencraft-engine --example export -- /tmp/demo/Demo.screencraft out.mp4 h264 1080 30
cargo run --release -p screencraft-audio --example mic_probe -- <out_dir> 6    # microphone capture check
python3 scripts/gen-notices.py          # refresh THIRD-PARTY-NOTICES.md after dependency changes
```

Build with `--features vp9` (and `VPX_STATIC=1`, libvpx installed) to enable VP9.

Environment variables for testing: `SCREENCRAFT_RECORDINGS_DIR` (library folder),
`SCREENCRAFT_OPEN` (open a package at start), `SCREENCRAFT_AUTORECORD=<seconds>` (record the main
display, report, quit).

## License

This project is licensed under the MIT License. By using this software, you agree that the authors are not liable for any issues, damages, or claims arising from its use.

See [LICENSE](LICENSE) for the full text, [THIRD-PARTY-NOTICES.md](THIRD-PARTY-NOTICES.md) for the open-source
components ScreenCraft uses, and [COMPLIANCE.md](COMPLIANCE.md) for the legal and compliance guide.

## Known limitations

- Windows and Linux are much less tested than macOS. The Windows H.264 path (Media Foundation) is new.
- Linux has no operating-system H.264, so recordings cannot be edited there yet; click and key
  logging works only on X11.
- Adding footage to a project re-encodes its whole screen video; camera video of the added part is
  not carried over, and camera-only projects can't be added to yet.
- HEVC export / compression is not built; the compressor reads H.264 MP4s and carries Opus audio only.
- Large sources (5K+) decode slower than real time when scrubbing.
- No code signing or notarisation; macOS and Windows will warn on first launch.
