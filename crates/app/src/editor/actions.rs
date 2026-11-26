//! Editor actions that reach outside the project document: idle detection, captions, SRT files,
//! music files.

use super::{ops, EditorState, Sel};
use crate::srt;
use std::time::Instant;

impl EditorState {
    /// Mark the project changed without an undo step (for automatic fix-ups).
    pub fn touch(&mut self) {
        self.dirty_since = Some(Instant::now());
        self.track_dirty = true;
        self.project_rev = self.project_rev.wrapping_add(1);
    }

    /// Idle stretches `[start, end]` in source ms.
    pub fn find_idle(&mut self, min_idle_ms: f64, keep_speech: bool) -> Vec<[f64; 2]> {
        let dur = self.project.media.duration_ms;
        let speech = if keep_speech { self.speech_flags() } else { None };
        screencraft_core::pace::idle_segments(&self.input, dur, speech.as_deref(), min_idle_ms.max(500.0), 250.0)
    }

    /// Find the silences in the speech and turn them into jump cuts: stretches that stay on the
    /// timeline, greyed out, but are skipped when it plays and left out of the export. Replaces
    /// any jump cuts made before. Returns how many were made and how long they run together.
    pub fn auto_jump_cuts(&mut self, settings: screencraft_core::jumpcut::JumpCutSettings) -> Result<(usize, f64), String> {
        use screencraft_core::jumpcut::{apply_jump_cuts, clear_jump_cuts, cut_total_ms, find_silences};
        let levels = screencraft_engine::jumpcuts::audio_levels(&self.package, &self.project)?;
        let ranges = find_silences(&levels, screencraft_engine::jumpcuts::FRAME_MS, &settings);
        let mut ids: Vec<String> = (0..ranges.len() * 2 + 2).map(|_| self.new_id("slice")).collect();
        let mut made = 0;
        let mut total = 0.0;
        self.edit(|p| {
            clear_jump_cuts(&mut p.scene.slices);
            made = apply_jump_cuts(&mut p.scene.slices, &ranges, || ids.pop().unwrap_or_else(|| "slice-x".into()));
            total = cut_total_ms(&p.scene.slices);
        });
        self.select(Sel::None);
        Ok((made, total))
    }

    /// Turn every jump cut back on.
    pub fn clear_jump_cuts(&mut self) {
        self.edit(|p| screencraft_core::jumpcut::clear_jump_cuts(&mut p.scene.slices));
        self.select(Sel::None);
    }

    /// Switch the selected clip (or, with nothing selected, the clip under the playhead) between
    /// playing and being a jump cut.
    pub fn toggle_jump_cut(&mut self) {
        let id = match &self.sel {
            Sel::Slice(id) => Some(id.clone()),
            _ => {
                let (_, idx) = ops::out_to_src(&self.project.scene.slices, self.playhead_ms);
                self.project.scene.slices.get(idx).map(|s| s.id.clone())
            }
        };
        let Some(id) = id else { return };
        self.edit(|p| {
            if let Some(s) = p.scene.slices.iter_mut().find(|s| s.id == id) {
                s.disabled = !s.disabled;
            }
        });
        self.sel = Sel::Slice(id);
    }

    /// Play one hit of a click / typing sound (when the style is picked in the Audio panel).
    pub fn preview_sound(&mut self, kind: &str, style: &str, volume: f64) {
        use screencraft_engine::audio_mix::{Extras, Soundtrack};
        self.pause();
        let set = screencraft_engine::sfx::load_set(kind, style);
        let mut hits = screencraft_engine::sfx::place(&set, &[(0.0, 0)]);
        for h in hits.iter_mut() {
            h.gain *= (self.project.config.audio.volume * 2.0 * volume) as f32;
        }
        let st = Soundtrack::new(&self.project, vec![]).with_extras(Extras { hits, ..Default::default() });
        if self.audio.is_none() {
            self.audio = super::playback::AudioPlayback::new();
        }
        if let Some(a) = self.audio.as_mut() {
            a.play(st, 0.0);
        }
    }

    /// Per-100 ms "someone is talking" flags from the microphone track.
    fn speech_flags(&self) -> Option<Vec<bool>> {
        let ranges = screencraft_engine::mic_speech(&self.package, &self.project)?;
        let n = (self.project.media.duration_ms / 100.0).ceil() as usize + 1;
        let mut flags = vec![false; n];
        for r in ranges {
            let (a, b) = ((r[0].max(0.0) / 100.0) as usize, (r[1].max(0.0) / 100.0) as usize);
            for f in flags.iter_mut().take(b + 1).skip(a) {
                *f = true;
            }
        }
        Some(flags)
    }

    #[allow(dead_code)]
    fn speech_flags_old(&self) -> Option<Vec<bool>> {
        let track = self.project.media.mic_track?;
        let path = self.package.screen_path(&self.project);
        let peaks = super::waveform_peaks(&path, track as usize).ok()?;
        // 20 ms packets; judge against the room's noise floor (20th percentile)
        let mut sorted = peaks.clone();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let floor = sorted.get(sorted.len() / 5).copied().unwrap_or(0.0);
        let thr = (floor * 3.0).max(0.04);
        let n = (peaks.len() / 5) + 1;
        let mut flags = vec![false; n];
        for (i, p) in peaks.iter().enumerate() {
            if *p > thr {
                flags[i / 5] = true;
            }
        }
        // hold 300 ms after speech
        let mut out = flags.clone();
        for i in 0..n {
            if flags[i] {
                for o in out.iter_mut().skip(i).take(4) {
                    *o = true;
                }
            }
        }
        Some(out)
    }

    pub fn ease_all(&mut self, ramp_ms: f64) -> usize {
        let mut ids: Vec<String> = (0..64).map(|_| self.new_id("ramp")).collect();
        let mut made = 0;
        self.edit(|p| {
            let mut next = || ids.pop().unwrap_or_else(|| "ramp-x".into());
            made = ops::ease_speed_changes(&mut p.scene.slices, ramp_ms, None, &mut next);
        });
        made
    }

    pub fn freeze_at_playhead(&mut self, hold_ms: f64) -> bool {
        let mut ids: Vec<String> = (0..3).map(|_| self.new_id("freeze")).collect();
        let t = self.playhead_ms;
        let mut out = None;
        self.edit(|p| {
            let mut next = || ids.pop().unwrap_or_else(|| "freeze-x".into());
            out = ops::freeze_at(&mut p.scene.slices, t, hold_ms, &mut next);
        });
        if let Some(id) = out {
            self.select(Sel::Slice(id));
            true
        } else {
            false
        }
    }

    pub fn import_srt(&mut self) {
        let Some(path) = rfd::FileDialog::new().add_filter("Subtitles", &["srt", "vtt"]).pick_file() else { return };
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) => {
                self.error = Some(format!("Could not read that file: {e}"));
                return;
            }
        };
        let items = srt::parse(&text);
        if items.is_empty() {
            self.error = Some("No captions found in that file.".into());
            return;
        }
        // SRT times describe the finished video, so map them back onto the recording
        let slices = self.project.scene.slices.clone();
        let ids: Vec<String> = items.iter().map(|_| self.new_id("cap")).collect();
        self.edit(|p| {
            for (it, id) in items.iter().zip(ids) {
                let s = ops::out_to_src(&slices, it.start).0;
                let e = ops::out_to_src(&slices, it.end).0;
                p.scene.subtitles.push(screencraft_core::project::Subtitle {
                    id,
                    text: it.text.clone(),
                    source_start_ms: s,
                    source_end_ms: e.max(s + 200.0),
                    word_starts: vec![],
                    word_ends: vec![],
                });
            }
        });
        self.select(Sel::None);
    }

    pub fn export_srt(&mut self) {
        let slices = self.project.scene.slices.clone();
        let to_eff = |src_ms: f64| -> Option<f64> {
            let disp = ops::src_to_out(&slices, src_ms)?;
            Some(screencraft_core::timeline::display_to_effective(&slices, disp))
        };
        let mut items: Vec<srt::Item> = self
            .project
            .scene
            .subtitles
            .iter()
            .filter_map(|c| {
                let spans = screencraft_core::transcript::word_spans(c);
                if spans.is_empty() {
                    return None;
                }
                let kept: Vec<(String, f64, f64)> = spans
                    .iter()
                    .filter(|(_, s, e)| !screencraft_core::transcript::is_range_cut(&slices, *s, *e))
                    .cloned()
                    .collect();
                if kept.is_empty() {
                    return None;
                }
                let all_kept = kept.len() == spans.len();
                let src_s = if all_kept { c.source_start_ms } else { kept[0].1 };
                let src_e = if all_kept { c.source_end_ms } else { kept[kept.len() - 1].2 };
                let s = to_eff(src_s).or_else(|| to_eff(kept[0].1))?;
                let e = to_eff(src_e).or_else(|| to_eff(kept[kept.len() - 1].2))?.max(s + 100.0);
                let text = if all_kept {
                    c.text.clone()
                } else {
                    kept.into_iter().map(|(w, _, _)| w).collect::<Vec<_>>().join(" ")
                };
                (!text.trim().is_empty()).then_some(srt::Item { start: s, end: e, text })
            })
            .collect();
        if items.is_empty() {
            self.error = Some("There are no captions to export.".into());
            return;
        }
        items.sort_by(|a, b| a.start.partial_cmp(&b.start).unwrap_or(std::cmp::Ordering::Equal));
        let base = self.project.name.replace(['/', '\\', ':'], "-");
        let Some(path) = rfd::FileDialog::new().set_file_name(format!("{base}.srt")).add_filter("SRT", &["srt"]).save_file() else { return };
        if let Err(e) = std::fs::write(path, srt::write(&items)) {
            self.error = Some(format!("Could not save: {e}"));
        }
    }

    /// Cut source ranges out of the timeline as non-destructive jump cuts.
    pub fn cut_source_ranges(&mut self, ranges: &[[f64; 2]]) {
        if ranges.is_empty() {
            return;
        }
        let mut ids: Vec<String> = (0..ranges.len() * 2 + 4).map(|_| self.new_id("slice")).collect();
        self.edit(|p| {
            screencraft_core::transcript::cut_ranges(&mut p.scene.slices, ranges, || ids.pop().unwrap_or_else(|| "slice-x".into()));
        });
    }

    /// Restore previously cut source ranges back into active playback.
    pub fn restore_source_ranges(&mut self, ranges: &[[f64; 2]]) {
        if ranges.is_empty() {
            return;
        }
        let mut ids: Vec<String> = (0..ranges.len() * 2 + 4).map(|_| self.new_id("slice")).collect();
        self.edit(|p| {
            screencraft_core::transcript::restore_ranges(&mut p.scene.slices, ranges, || ids.pop().unwrap_or_else(|| "slice-x".into()));
        });
    }

    /// Start whisper.cpp on this recording's microphone.
    pub fn start_captions(&mut self) {
        let (tx, rx) = std::sync::mpsc::channel();
        let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        self.panel.caps.cancel = cancel.clone();
        self.panel.caps.rx = Some(rx);
        self.panel.caps.busy = true;
        self.panel.caps.msg = "Starting…".into();
        self.panel.caps.frac = None;
        crate::whisper::spawn(
            tx,
            crate::whisper::Request {
                package: self.package.clone(),
                project: self.project.clone(),
                model: self.panel.caps.model.clone(),
                language: self.panel.caps.language.clone(),
                translate: self.panel.caps.translate,
                cancel,
            },
        );
    }

    /// Fold finished caption generation into the project.
    pub fn take_caption_results(&mut self) {
        let Some(done) = self.panel.caps.done.take() else { return };
        match done {
            Err(e) if e == "cancelled" => {}
            Err(e) => self.error = Some(format!("Caption generation failed: {e}")),
            Ok(caps) if caps.is_empty() => self.error = Some("No speech was found in this recording.".into()),
            Ok(caps) => {
                let ids: Vec<String> = caps.iter().map(|_| self.new_id("cap")).collect();
                let first = self.project.scene.subtitles.is_empty();
                self.edit(|p| {
                    if first {
                        super::panels::caption_preset("classic", &mut p.config.subtitles);
                    }
                    for (c, id) in caps.into_iter().zip(ids) {
                        p.scene.subtitles.push(screencraft_core::project::Subtitle {
                            id,
                            text: c.text,
                            source_start_ms: c.start_ms,
                            source_end_ms: c.end_ms,
                            word_starts: c.word_starts,
                            word_ends: c.word_ends,
                        });
                    }
                });
                self.panel.caps.view_transcript = true;
                self.panel.caps.selected_words.clear();
                self.panel.caps.anchor_word = None;
                self.select(Sel::None);
            }
        }
    }

    pub fn add_music(&mut self) {
        let Some(path) = rfd::FileDialog::new().add_filter("Audio", &["mp3", "m4a", "aac", "wav", "aif", "aiff", "caf", "flac", "ogg"]).pick_file() else { return };
        match crate::music::import(&self.package, &path) {
            Ok((file, name, dur)) => {
                let id = self.new_id("music");
                let t = self.playhead_ms.round();
                self.edit(|p| {
                    p.scene.music_clips.push(screencraft_core::project::MusicClip {
                        id: id.clone(),
                        file,
                        name,
                        offset_ms: t,
                        in_ms: 0.0,
                        dur_ms: dur,
                        file_dur_ms: dur,
                        fade_in_ms: 0.0,
                        fade_out_ms: 0.0,
                    })
                });
                self.select(Sel::Music(id));
                self.tab = super::Tab::Audio;
            }
            Err(e) => self.error = Some(e),
        }
    }
}
