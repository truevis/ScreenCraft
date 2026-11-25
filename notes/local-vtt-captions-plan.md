# Plan: captions from the local aifab-vtt engine (2026-11-25)

Status: plan only. No ScreenCraft code has changed.

## Goal

Generate ScreenCraft captions with the same local speech-to-text engine as `C:\GitHub\aifab-vtt`: faster-whisper `large-v3-turbo` on CUDA float16, the same `custom_words.txt` vocabulary, fully offline. The current captions path is `crates/app/src/whisper.rs` (whisper.cpp, CPU, its own model download). The new engine is faster on an NVIDIA GPU, more accurate, and shares the vocabulary you already maintain.



## Why not call `toggle.ps1` directly

`toggle.ps1` starts or stops the **dictation app** (`python -m vtt.app`): a hotkey listener that records the mic and pastes text into the focused window. It has no way to take an audio file and return timed captions, and toggling it would also grab Ctrl+Space and mute the speakers. ScreenCraft should reuse the engine (venv, model, `vtt/asr.py`, `custom_words.txt`), not the toggle script.

## Design

### 1. A batch entry point in aifab-vtt (`vtt/captions.py`)

```
python -m vtt.captions --in <audio.wav> --out <captions.json> [--language en] [--prompt-file custom_words.txt]
```
- Loads `WhisperModel` the way `vtt/asr.py` does, using the `[asr]` settings in `config.toml`.
- Calls `model.transcribe(audio, word_timestamps=True, vad_filter=True, initial_prompt=<terms>)`.
- Writes JSON: `{ "segments": [ { "start_ms", "end_ms", "text", "words": [ {"start_ms","end_ms","word","p"} ] } ] }`.
- Prints `progress <0-100>` lines to stdout (segment end / duration) so ScreenCraft can show a percentage.
- Applies the same `postprocess.py` replacements (`spoken => Written`) and casing fixes as dictation, but **not** the trailing-period rule: captions keep their punctuation.
- Exit code 0 on success; the error goes to stderr otherwise.

This is the only change to aifab-vtt. The toggle and the dictation app stay as they are.

### 2. ScreenCraft side: a second captions engine

New module `crates/app/src/vtt_local.rs`, next to `whisper.rs`:
1. **Discover** the engine: a `Prefs` field `local_vtt_dir` (default `C:\GitHub\aifab-vtt`). It's usable when `<dir>\.venv\Scripts\python.exe` and `<dir>\vtt\captions.py` exist.
2. **Export audio**: mix the mic track (fall back to system audio) to 16 kHz mono WAV in the scratch/package folder. The decode and resample helpers in `screencraft-audio` (`decode.rs`, `mix.rs`) already exist.
3. **Run** `python.exe -m vtt.captions --in … --out …` with `current_dir = local_vtt_dir` and `CREATE_NO_WINDOW` (no console flash). Read the `progress` lines, map them to `CapEvent::Progress`, and allow Cancel (kill the child).
4. **Import** the JSON into the existing caption model, using the same structure `srt.rs` and the Whisper path produce (`source_start_ms` and so on), so the timeline, editing, SRT export and burn-in work unchanged. Word timings make finer caption splitting possible later (karaoke-style highlight).
5. **Errors**: if the engine is missing, the GPU is unavailable, or CUDA runs out of memory, show the stderr text and offer the built-in whisper.cpp engine as a fallback.

### 3. UI (Captions panel)

- An "Engine" segmented control: **Local GPU (aifab-vtt)**, or **Built-in (whisper.cpp)**. Local GPU is the default when it's detected.
- Settings: engine folder picker, and an "Open custom words" button that opens `custom_words.txt`.
- No model download UI for the local engine; aifab-vtt's `setup.bat` owns its models.

### 4. GPU contention

If the dictation app is running, its model already uses about 3–4 GB of VRAM, and a second model instance will roughly double that. Option A (simple): just run the batch process; it's fine on cards with 8 GB or more. Option B (later): add a localhost or named-pipe "transcribe file" request to `vtt.app` so one loaded model serves both. Start with A.

## Steps

1. aifab-vtt: add `vtt/captions.py` and test it on a WAV from a ScreenCraft recording (compare with whisper.cpp output).
2. ScreenCraft: WAV export helper + `vtt_local.rs` runner + JSON → captions import, with a unit test on a sample JSON.
3. Captions panel engine switch + prefs field; fallback to whisper.cpp.
4. Build through the remote GitHub Actions workflow (not locally, per AGENTS.md) and test end to end: record with the mic → Generate captions → check timing on the timeline → export the MP4 with burned-in captions.
5. Optional: a shared model through the running dictation app (option B).

## Risks

- aifab-vtt is a separate local repo with no installer, so the feature only works where it's set up. Keep whisper.cpp as the fallback, and don't remove it. 
- Python and CUDA cold start is about 5–15 s while the model loads. Show "Loading model…" until the first progress line.
- Paths with spaces: pass arguments as a vector (`std::process::Command::arg`), never a joined string.
