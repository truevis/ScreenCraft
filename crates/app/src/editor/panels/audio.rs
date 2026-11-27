use super::*;
use crate::editor::Sel;
use screencraft_audio::loudness::{gain_db, BROADCAST_LUFS, PODCAST_LUFS, YOUTUBE_LUFS};
use screencraft_engine::LoudnessReport;
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

const CLICK_STYLES: [(&str, &str); 3] = [("soft", "Soft"), ("crisp", "Crisp"), ("classic", "Classic")];
const KEY_STYLES: [(&str, &str); 3] = [("mechanical", "Mechanical"), ("soft", "Soft"), ("thock", "Thock")];

pub struct AudioPanelState {
    pub busy: bool,
    pub in_flight_key: Option<u64>,
    pub current_key: Option<u64>,
    pub pending_key: Option<(u64, Instant)>,
    pub last_report: Option<(u64, LoudnessReport)>,
    pub last_checked_rev: u64,
    pub just_finished: bool,
    pub error: Option<String>,
    pub rx: Option<Receiver<(u64, Result<LoudnessReport, String>)>>,
}

impl Default for AudioPanelState {
    fn default() -> Self {
        Self {
            busy: false,
            in_flight_key: None,
            current_key: None,
            pending_key: None,
            last_report: None,
            last_checked_rev: u64::MAX,
            just_finished: false,
            error: None,
            rx: None,
        }
    }
}

impl AudioPanelState {
    pub fn poll(&mut self, ctx: &egui::Context) {
        let mut finished = false;
        if let Some(rx) = &self.rx {
            while let Ok((key, res)) = rx.try_recv() {
                finished = true;
                match res {
                    Ok(rep) => {
                        self.last_report = Some((key, rep));
                        self.error = None;
                    }
                    Err(e) => {
                        self.error = Some(e);
                    }
                }
            }
            if self.busy {
                ctx.request_repaint_after(Duration::from_millis(80));
            }
        }
        if finished {
            self.busy = false;
            self.in_flight_key = None;
            self.rx = None;
            self.just_finished = true;
        }
    }
}

pub fn show(st: &mut EditorState, ui: &mut Ui) {
    let a = st.project.config.audio.clone();
    let m = st.project.media.clone();
    kit::h3(ui, "Audio", true);
    if m.mic.is_none() && !m.has_microphone && !m.has_system_audio {
        note(ui, "No audio was recorded. Enable the microphone or system audio in the recorder toolbar.");
    }
    ui.add_enabled_ui(!a.loudness_enabled, |ui| {
        slider(st, ui, "a-master", "Master volume", a.volume, (0.0, 1.0, 0.01), pct, |p, v| p.config.audio.volume = v);
    });
    if a.loudness_enabled {
        kit::hint(ui, "Master level is controlled automatically by Loudness target.");
    }
    slider(st, ui, "a-mic", "Microphone", a.mic_volume, (0.0, 1.0, 0.01), pct, |p, v| p.config.audio.mic_volume = v);
    if m.mic.is_none() && m.mic_track.is_none() {
        kit::hint(ui, "This recording has no separate microphone track (it was recorded before microphones were split out), so the Device slider controls everything.");
    }
    slider(st, ui, "a-dev", "Device audio", a.device_volume, (0.0, 1.0, 0.01), pct, |p, v| p.config.audio.device_volume = v);

    kit::h3_help(
        ui,
        "Noise reduction",
        false,
        "Attenuates steady background noise such as cooling fans, hum, and room hiss using an 80 Hz rumble filter and spectral suppression. Applied non-destructively during preview playback and export.",
    );
    toggle(
        st,
        ui,
        "Reduce background noise",
        Some("Cuts steady fan noise, hum, and room hiss."),
        a.noise_reduction,
        |p, v| p.config.audio.noise_reduction = v,
    );
    if a.noise_reduction {
        slider_def(
            st,
            ui,
            "a-nr-strength",
            "Strength",
            a.noise_reduction_strength,
            (0.0, 1.0, 0.05),
            0.65,
            pct,
            |p, v| p.config.audio.noise_reduction_strength = v,
        );
        if m.mic.is_none() && m.mic_track.is_none() && m.has_system_audio {
            kit::hint(ui, "Applied to device audio (this recording has no separate microphone track).");
        }
    }

    kit::h3_help(
        ui,
        "Loudness target",
        false,
        "Measures integrated loudness (ITU-R BS.1770-4 LUFS) across your edited mix and brings it to your target with a -1.0 dBTP true-peak ceiling limiter. Applied non-destructively during preview playback (once analysis completes) and export.",
    );
    toggle(
        st,
        ui,
        "Match loudness target",
        Some("Normalizes the mix to the right level for YouTube (-14 LUFS) without clipping."),
        a.loudness_enabled,
        |p, v| p.config.audio.loudness_enabled = v,
    );
    if a.loudness_enabled {
        let preset_cur = if (a.loudness_lufs - YOUTUBE_LUFS).abs() <= 0.25 {
            "youtube".to_string()
        } else if (a.loudness_lufs - PODCAST_LUFS).abs() <= 0.25 {
            "podcast".to_string()
        } else if (a.loudness_lufs - BROADCAST_LUFS).abs() <= 0.25 {
            "broadcast".to_string()
        } else {
            "custom".to_string()
        };
        let presets = [
            ("youtube".to_string(), "YouTube"),
            ("podcast".to_string(), "Podcast"),
            ("broadcast".to_string(), "Broadcast"),
        ];
        seg(st, ui, Some("Preset"), &presets, preset_cur, |p, v| {
            p.config.audio.loudness_lufs = match v.as_str() {
                "podcast" => PODCAST_LUFS,
                "broadcast" => BROADCAST_LUFS,
                _ => YOUTUBE_LUFS,
            };
        });
        slider_def(
            st,
            ui,
            "a-lufs-target",
            "Target loudness",
            a.loudness_lufs,
            (-24.0, -8.0, 0.5),
            YOUTUBE_LUFS,
            |v| format!("{v:.1} LUFS"),
            |p, v| p.config.audio.loudness_lufs = v,
        );

        let status = if a.volume <= 0.0 {
            "Master volume is muted (0%).".to_string()
        } else if st.panel.audio.busy {
            "Analyzing soundtrack loudness…".to_string()
        } else if let Some(err) = &st.panel.audio.error {
            format!("Loudness analysis failed: {err}")
        } else if let (Some(cur_key), Some((rep_key, rep))) = (st.panel.audio.current_key, st.panel.audio.last_report) {
            if cur_key == rep_key && st.panel.audio.last_checked_rev == st.project_rev {
                if let Some(lufs) = rep.integrated_lufs {
                    let delta = gain_db(lufs, a.loudness_lufs);
                    let sign = if delta >= 0.0 { "+" } else { "" };
                    format!(
                        "Mix: {lufs:.1} LUFS → {:.1} LUFS ({sign}{delta:.1} dB) · -1.0 dBTP ceiling",
                        a.loudness_lufs
                    )
                } else {
                    "No audible audio found to normalize.".to_string()
                }
            } else {
                ui.ctx().request_repaint();
                "Waiting to analyze loudness…".to_string()
            }
        } else {
            ui.ctx().request_repaint();
            "Waiting to analyze loudness…".to_string()
        };
        kit::hint(ui, &status);
    }

    kit::h3_help(
        ui,
        "Sound effects",
        false,
        "Adds synthesized click and typing sounds at each logged input event. Mixed non-destructively live during preview playback and on export.",
    );
    toggle(st, ui, "Mouse click sounds", Some("Plays a click at every click in the recording."), a.click_sounds, |p, v| p.config.audio.click_sounds = v);
    if a.click_sounds {
        let opts: Vec<(String, &str)> = CLICK_STYLES.iter().map(|(a, b)| (a.to_string(), *b)).collect();
        let before = a.click_style.clone();
        seg(st, ui, Some("Click style"), &opts, a.click_style.clone(), |p, v| p.config.audio.click_style = v);
        if st.project.config.audio.click_style != before {
            let (style, vol) = (st.project.config.audio.click_style.clone(), a.click_volume);
            st.preview_sound("click", &style, vol);
        }
        slider(st, ui, "a-clickvol", "Click volume", a.click_volume, (0.0, 1.0, 0.01), pct, |p, v| p.config.audio.click_volume = v);
    }
    toggle(st, ui, "Keyboard typing sounds", Some("Plays a key sound for every key you pressed."), a.key_sounds, |p, v| p.config.audio.key_sounds = v);
    if a.key_sounds {
        let opts: Vec<(String, &str)> = KEY_STYLES.iter().map(|(a, b)| (a.to_string(), *b)).collect();
        let before = a.key_style.clone();
        seg(st, ui, Some("Keyboard style"), &opts, a.key_style.clone(), |p, v| p.config.audio.key_style = v);
        if st.project.config.audio.key_style != before {
            let (style, vol) = (st.project.config.audio.key_style.clone(), a.key_volume);
            st.preview_sound("key", &style, vol);
        }
        slider(st, ui, "a-keyvol", "Typing volume", a.key_volume, (0.0, 1.0, 0.01), pct, |p, v| p.config.audio.key_volume = v);
    }

    kit::h3_help(ui, "Music", false, "Drag a green block to place it, drag its ends to trim, and press S to split at the playhead.");
    let clips = st.project.scene.music_clips.clone();
    if !clips.is_empty() && (m.mic.is_some() || m.has_microphone) {
        toggle(st, ui, "Lower music when I speak", Some("Ducks the music under your microphone."), a.duck_music, |p, v| p.config.audio.duck_music = v);
        if a.duck_music {
            slider(st, ui, "duck-level", "Music while speaking", a.duck_level, (0.05, 0.8, 0.05), pct, |p, v| p.config.audio.duck_level = v);
        }
    }
    let music_ids = st.selected_music_ids();
    let sel = if let Sel::Music(id) = &st.sel { clips.iter().find(|c| &c.id == id).cloned() } else { None };
    if let Some(c) = &sel {
        let id = c.id.clone();
        let ids_in = music_ids.clone();
        slider(st, ui, &format!("fade-in-{id}"), "Fade in", c.fade_in_ms / 1000.0, (0.0, 5.0, 0.1), |v| format!("{v:.1} s"), move |p, v| {
            for x in p.scene.music_clips.iter_mut().filter(|x| ids_in.contains(&x.id)) {
                x.fade_in_ms = (v * 1000.0).round();
            }
        });
        let ids_out = music_ids.clone();
        slider(st, ui, &format!("fade-out-{id}"), "Fade out", c.fade_out_ms / 1000.0, (0.0, 5.0, 0.1), |v| format!("{v:.1} s"), move |p, v| {
            for x in p.scene.music_clips.iter_mut().filter(|x| ids_out.contains(&x.id)) {
                x.fade_out_ms = (v * 1000.0).round();
            }
        });
    }
    if !clips.is_empty() {
        slider(st, ui, "a-music", "Music volume", a.music_volume, (0.0, 1.0, 0.01), pct, |p, v| p.config.audio.music_volume = v);
    }
    if kit::btn_line(ui, "Add music…", false).clicked() {
        st.add_music();
    }
    if sel.is_some() {
        let n = music_ids.len();
        let del_label = if n > 1 { format!("Delete {n} music clips") } else { "Delete selected music clip".to_string() };
        if kit::btn_line(ui, &del_label, true).clicked() {
            st.delete_selected();
        }
    }
}
