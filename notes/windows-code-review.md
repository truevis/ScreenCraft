# ScreenCraft: Windows code review (2026-11-25)

Scope: Windows-only paths on branch `ci/windows-installer` (4d6e311). These notes come from reading the code. Nothing was compiled or run locally (see AGENTS.md). Ordered by impact.

## 1. Encoding (`crates/encode/src/mf.rs`)

- **Software encoder only.** `MfH264Encoder::new` calls `CoCreateInstance(CLSID_MSH264EncoderMFT)`, which is Microsoft's CPU H.264 encoder. On NVIDIA, AMD or Intel machines, use `MFTEnumEx(MFT_CATEGORY_VIDEO_ENCODER, MFT_ENUM_FLAG_HARDWARE | MFT_ENUM_FLAG_SORTANDFILTER, …)` to pick NVENC, AMF or QSV, and fall back to the software MFT when none is found. This should be the biggest speed and CPU win for 1440p/4K exports and recordings.
  - Hardware MFTs are usually **async**: they need `MF_TRANSFORM_ASYNC_UNLOCK` and an `IMFMediaEventGenerator` loop (`METransformNeedInput` / `METransformHaveOutput`). The current synchronous `ProcessInput` → `drain` loop will not drive them as written, so plan for a second code path.
- **CPU NV12 conversion per frame.** Each frame goes from BGRA on the GPU to a CPU staging copy, then to I420, then to NV12, then to an `IMFSample` from a byte copy. A hardware MFT with `MF_SA_D3D11_AWARE` plus an `IMFDXGIDeviceManager` could take D3D11 textures directly. That is a larger refactor; do it after the item above.
- **Rate control isn't set.** Only `MF_MT_AVG_BITRATE` is set. Set `CODECAPI_AVEncCommonRateControlMode` (quality-VBR for exports, CBR/peak-constrained for recording) and `CODECAPI_AVEncMPVGOPSize` through `ICodecAPI`, so seeking in the editor gets predictable keyframes.
- **`MF_LOW_LATENCY` is a hint.** The last commit added reorder and renegotiation handling, which is good. Add a regression test in `crates/encode/tests/roundtrip.rs` gated on `cfg(windows)`, so the CI workflow actually runs encode→decode on the Windows runner.
- **COM init.** `init()` calls `CoInitializeEx(MTA)` once, from whichever thread gets there first. COM apartments are per-thread, so encoder and decoder threads created later are not initialised. They work today only through the implicit MTA. Call `CoInitializeEx` on every worker thread that touches MF (a small RAII guard), and never call `MFShutdown` while workers live.

## 2. Screen capture (`crates/capture/src/backend_windows.rs`)

- **Desktop Duplication fails on hybrid-GPU laptops.** `find_output` errors when the display hangs off the other adapter (common on Optimus laptops) and in RDP sessions. Add a **Windows.Graphics.Capture** (`GraphicsCaptureItem` / `Direct3D11CaptureFramePool`) backend, either as the default on Win10 1903+ or as a fallback when duplication fails. It also enables:
  - per-window capture (follow a window instead of a fixed area);
  - `IsCursorCaptureEnabled = false`, which matters because the cursor is re-rendered from the input log;
  - `IsBorderRequired = false` on Win11 (removes the yellow border).
- **Full-output readback every frame.** `next()` copies the whole output to staging even when capturing a small area or a 9:16 crop. Use `CopySubresourceRegion` with the area box to cut bandwidth and memory. Reuse the `Vec<u8>` (pool) instead of `.to_vec()` per frame, which allocates about 33 MB per 4K frame at 60 fps.
- **Rotation and HDR.** `DXGI_OUTDUPL_DESC.Rotation` is ignored, so portrait monitors will come out sideways. HDR desktops return frames that look washed out unless `IDXGIOutput5::DuplicateOutput1` is used with an `R16G16B16A16_FLOAT` / tone-map path, or WGC with SDR conversion.
- **ACCESS_LOST loop** (lines ~249–254) retries forever with sleeps. Cap the retries, or send an `Event` to the UI ("capture interrupted: UAC / lock screen") so the user isn't left recording black frames silently.

## 3. Input hooks (`crates/capture/src/input/windows.rs`)

- **Privacy: the LL keyboard hook records every key, passwords included.** Keystrokes go into the project's `input.json` and can be burned into a video by the shortcut overlay. Suggestions:
  - skip characters when the focused control is a password field (UIA `IsPassword`, or `ES_PASSWORD` on the focused edit);
  - store only modifier+key combos by default, with plain typing opt-in;
  - say clearly in the UI that keys are recorded.
- **Polling the mouse at ~120 Hz with `GetAsyncKeyState`** can miss short clicks shorter than the poll interval, such as touchpad taps. A `WH_MOUSE_LL` hook already fits the existing hook thread and gives exact click timestamps.
- `HOOK_TARGET.lock().unwrap()` inside the hook callback: a poisoned mutex would panic inside an `extern "system"` callback, which aborts the process. Use `lock().unwrap_or_else(|e| e.into_inner())`, or `try_lock` and drop the event; a hook must never block or panic anyway, because Windows silently removes slow LL hooks (`LowLevelHooksTimeout`).

## 4. Packaging and integration

- **No application manifest or version resource.** `crates/app` has no `build.rs`, `winres` or `embed-resource`. Add one with:
  - `PerMonitorV2` DPI awareness, set declaratively so Windows doesn't bitmap-scale the area picker before winit opts in;
  - `longPathAware`;
  - the exe icon and version info, which SmartScreen and Add/Remove Programs show.
- **Code signing.** The NSIS installer from the workflow is unsigned, so SmartScreen will warn. Add an Azure Trusted Signing / signtool step behind a secret.
- **Workflow pin guard** (`build-windows.yml`) only allows `mf.rs` and the workflow file to differ from the pinned SHA. That blocks any of the fixes above from being built. Widen the allow-list, or switch to building `HEAD` once the branch is trusted.
- **`.wrangler/cache/*.json` is committed** and includes a Cloudflare account id and a named account. Remove it and add it to `.gitignore`. This isn't Windows-specific, but you'll want it gone before this branch goes public.

## 5. Robustness (all platforms, but felt most on Windows)

- **Non-atomic writes.** `prefs.rs::save` and `dialogs.rs:85` use `fs::write` directly. Antivirus or OneDrive locking, or a crash mid-write, leaves an empty `settings.json`, and `Prefs::load` then silently resets to defaults. Write to a `.tmp` file then rename, as `engine/src/package.rs` already does, and log failures instead of `let _ =`.
- **`partial_cmp(..).unwrap()` in 15 sorts** (e.g. `editor/actions.rs:105`, `timeline.rs:340`, `captions.rs:104`). A NaN from a corrupted project or a divide-by-zero panics the UI thread. Use `f64::total_cmp`.
- **`Mutex::lock().unwrap()`** across the export and playback threads (`export_ui.rs`, `playback.rs`). One panicking worker poisons the lock and then crashes the UI. Switch to `parking_lot::Mutex` (no poisoning), or handle the poisoned case.
- **Recordings folder on OneDrive.** The default `dirs::video_dir()` is often OneDrive-synced on Windows, so syncing large `.mp4` files while they're still being written causes sharing violations. Consider `%LOCALAPPDATA%` for in-progress recordings, or detect OneDrive and warn.
- **Whisper download** (`app/src/whisper.rs`) checks only `size >= approx/2`. Pin SHA-256s for the five models and verify them before the rename.

## 6. Remove the macOS and Linux code (repo is now Windows-only)

You can delete these, or cut them down to their Windows half:
- `crates/capture/src/backend_macos.rs`, `backend_linux.rs`, `input/macos.rs`, `input/linux.rs`, `input/ax.rs`
- `crates/encode/src/hw_videotoolbox.rs`, `vt_decode.rs`
- the `cfg(target_os = "macos" | "linux")` branches (27 `cfg` sites in total), plus `permissions.rs` and the permissions window, since Windows needs no screen-recording or input-monitoring grants
- the `[target.'cfg(target_os = …)']` dependency tables in each `Cargo.toml` (objc2, core-foundation, x11/pipewire, and so on)
- the macOS and Linux sections of README and TODO, plus the AV1/VP9 "Linux has no OS H.264" gap
- the ClipCraft sibling-model lookup in `whisper.rs`, if ClipCraft was a macOS app

Once that's done, `encode` and `capture` no longer need a backend trait. With a single implementation, the concrete Windows types can be called directly, which makes the code smaller and easier to follow. Make the deletions their own commit and build it with the remote workflow. That needs item 4's allow-list change first.

## Suggested order

1. Hardware MFT encoder + `ICodecAPI` rate control (largest user-visible gain).
2. Windows.Graphics.Capture fallback (fixes laptops and RDP, adds window capture).
3. Password-field keystroke filtering.
4. App manifest, signing, and a widened CI allow-list so items 1–3 can be built.
5. `total_cmp`, atomic writes, poison-safe locks.

## Status after the 2026-11-25 fix pass (branch `ci/windows-installer`)

Done:
- Stop-recording bug: the real encoder error is reported, and partial MP4s are finalized.
- MF: per-thread COM, `MF_E_NOTACCEPTING` retry, output buffer sizing, keyframe spacing.
- Log file at `%LOCALAPPDATA%\ScreenCraft\screencraft.log`, plus a panic hook.
- Keyboard hook: no blocking or panics, and typing in password boxes is skipped.
- NaN-safe sorts; atomic settings and presets saves.
- `.wrangler` cache removed; CI builds HEAD and runs the H.264 round-trip and recorder tests.

Not done yet (each needs its own build-and-test cycle):
- Hardware (async) MFT encoder and D3D11-aware input.
- Windows.Graphics.Capture backend, sub-rect copy, buffer pooling, rotation and HDR.
- `WH_MOUSE_LL` click hook.
- App manifest, code signing.
- Whisper SHA-256 pins.
- macOS/Linux code removal.
- OneDrive recordings handling.
