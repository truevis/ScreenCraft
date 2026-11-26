use super::annotate::note_row;
use super::*;
use crate::editor::{ops, Sel};
use crate::srt;
use crate::theme::*;
use crate::whisper::{CapEvent, Caption};
use eframe::egui::{Align2, Color32, CornerRadius, Pos2, Sense, Stroke, StrokeKind, Vec2};
use std::collections::{BTreeSet, HashMap};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Receiver;
use std::sync::Arc;

/// Style presets; position and width are left as the user set them, except TikTok's own.
pub fn preset(name: &str, s: &mut screencraft_core::project::SubtitleStyle) {
    screencraft_core::presets::apply_subtitle_preset(name, s);
}

const MODELS: [(&str, &str); 5] = [
    ("small.en", "Accurate · English · 488 MB"),
    ("large-v3-turbo-q5_0", "Most accurate · multilingual · 574 MB"),
    ("base.en", "Balanced · English · 148 MB"),
    ("tiny.en", "Fastest · English · 78 MB"),
    ("base", "Multilingual · 148 MB"),
];

pub struct CapState {
    pub model: String,
    pub language: String,
    pub translate: bool,
    pub view_transcript: bool,
    pub selected_words: BTreeSet<usize>,
    pub anchor_word: Option<usize>,
    pub search: String,
    pub busy: bool,
    pub msg: String,
    pub frac: Option<f32>,
    pub cancel: Arc<AtomicBool>,
    pub rx: Option<Receiver<CapEvent>>,
    pub done: Option<Result<Vec<Caption>, String>>,
}

impl Default for CapState {
    fn default() -> Self {
        Self {
            model: "small.en".into(),
            language: "auto".into(),
            translate: false,
            view_transcript: true,
            selected_words: BTreeSet::new(),
            anchor_word: None,
            search: String::new(),
            busy: false,
            msg: String::new(),
            frac: None,
            cancel: Arc::new(AtomicBool::new(false)),
            rx: None,
            done: None,
        }
    }
}

impl Drop for CapState {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

impl CapState {
    pub fn poll(&mut self, ctx: &egui::Context) {
        let mut finished = false;
        if let Some(rx) = &self.rx {
            while let Ok(ev) = rx.try_recv() {
                match ev {
                    CapEvent::Progress { msg, frac } => {
                        self.msg = msg;
                        self.frac = frac;
                    }
                    CapEvent::Done(r) => {
                        self.done = Some(r);
                        finished = true;
                    }
                }
            }
            if self.busy {
                ctx.request_repaint_after(std::time::Duration::from_millis(150));
            }
        }
        if finished {
            self.busy = false;
            self.rx = None;
            self.msg.clear();
            self.frac = None;
        }
    }
}

pub fn show(st: &mut EditorState, ui: &mut Ui) {
    let sty = st.project.config.subtitles.clone();
    if !matches!(sty.style.as_str(), "tiktok" | "classic" | "minimal") {
        st.edit(|p| preset("classic", &mut p.config.subtitles));
    }
    let mut list = st.project.scene.subtitles.clone();
    list.sort_by(|a, b| a.source_start_ms.partial_cmp(&b.source_start_ms).unwrap_or(std::cmp::Ordering::Equal));
    // on-device speech recognition (whisper.cpp); runs locally, nothing is uploaded
    kit::h3_help(ui, "Auto captions", true, "Transcribes the microphone on this computer with whisper.cpp. The first run downloads the model you pick (once); the audio never leaves your machine.");
    ui.add_space(6.0);
    kit::muted(ui, "Model");
    let cur_label = MODELS.iter().find(|m| m.0 == st.panel.caps.model).map(|m| m.1).unwrap_or("");
    let labels: Vec<&str> = MODELS.iter().map(|m| m.1).collect();
    if let Some(i) = kit::select(ui, "whisper-model", cur_label, &labels, ui.available_width() - 28.0) {
        st.panel.caps.model = MODELS[i].0.to_string();
        if st.panel.caps.model.ends_with(".en") {
            st.panel.caps.translate = false;
        }
    }
    if !st.panel.caps.model.ends_with(".en") {
        let langs: Vec<(String, &str)> = vec![("auto".into(), "Auto"), ("en".into(), "English"), ("es".into(), "Spanish"), ("fr".into(), "French"), ("de".into(), "German")];
        let mut l = st.panel.caps.language.clone();
        if kit::segmented(ui, Some("Language"), &langs, &mut l) {
            st.panel.caps.language = l;
        }
    }
    let mut tr = st.panel.caps.translate;
    if kit::toggle_row(
        ui,
        "Translate to English",
        Some("Translates non-English speech to English offline using a multilingual Whisper model."),
        &mut tr,
    ) {
        st.panel.caps.translate = tr;
        if tr && st.panel.caps.model.ends_with(".en") {
            st.panel.caps.model = if st.panel.caps.model == "small.en" {
                "large-v3-turbo-q5_0".into()
            } else {
                "base".into()
            };
        }
    }
    let busy = st.panel.caps.busy;
    if busy {
        let stopping = st.panel.caps.cancel.load(Ordering::Relaxed);
        let label = if stopping {
            "Stopping…".to_string()
        } else if let Some(f) = st.panel.caps.frac {
            format!("Stop transcribing · {:.0}%", (f * 100.0).clamp(0.0, 100.0))
        } else {
            "Stop transcribing".to_string()
        };
        if kit::btn_progress(ui, &label, st.panel.caps.frac, true).clicked() && !stopping {
            st.panel.caps.cancel.store(true, Ordering::Relaxed);
            st.panel.caps.msg = "Stopping…".into();
        }
    } else if kit::btn_line(ui, "Generate captions", false).clicked() {
        st.start_captions();
    }
    kit::h3_help(ui, "Manual captions", false, "Type captions or import an SRT file. They sit in the middle of the bottom third of the video, like TikTok.");
    let w = (ui.available_width() - 12.0) / 3.0;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        if kit::btn_line_w(ui, "Add caption", false, w).clicked() {
            st.add_caption();
        }
        if kit::btn_line_w(ui, "Import SRT…", false, w).clicked() {
            st.import_srt();
        }
        if kit::btn_line_w(ui, "Export SRT…", false, w).clicked() {
            st.export_srt();
        }
    });

    let cap = if let Sel::Caption(id) = &st.sel { list.iter().find(|c| &c.id == id).cloned() } else { None };
    let multi = st.selected_caption_ids();
    if cap.is_some() && multi.len() > 1 {
        kit::h3(ui, &format!("{} captions selected", multi.len()), false);
        kit::hint(ui, "Drag any selected caption to move them all together. Ctrl-click adds or removes one, Shift-click selects a range, Ctrl+A selects all.");
        if multi.len() < list.len() && kit::btn_line(ui, "Select all captions", false).clicked() {
            st.select_all_captions();
        }
        if kit::btn_line(ui, &format!("Delete {} captions", multi.len()), true).clicked() {
            st.delete_selected();
        }
        if kit::btn_line(ui, "Deselect", false).clicked() {
            st.select(Sel::None);
        }
    } else if let Some(c) = cap {
        kit::h3(ui, "Selected caption", false);
        ui.add_space(6.0);
        kit::muted(ui, "Text");
        let mut text = c.text.clone();
        if ui.add(egui::TextEdit::multiline(&mut text).desired_rows(3).desired_width(f32::INFINITY).margin(egui::Margin::symmetric(10, 8))).changed() {
            let id = c.id.clone();
            st.edit_keyed(Some(&format!("cap-{id}")), |p| {
                if let Some(x) = p.scene.subtitles.iter_mut().find(|x| x.id == id) {
                    x.text = text;
                }
            });
        }
        kit::hint(ui, &format!("{:.1} s. Drag the block on the Captions track to move it, its ends to trim.", (c.source_end_ms - c.source_start_ms) / 1000.0));
        if kit::btn_line(ui, "Delete caption", true).clicked() {
            st.delete_selected();
        }
    } else if !list.is_empty() {
        kit::h3(ui, &format!("{} caption{}", list.len(), if list.len() == 1 { "" } else { "s" }), false);
        kit::segmented(ui, None, &[(true, "Transcript"), (false, "Captions")], &mut st.panel.caps.view_transcript);
        if st.panel.caps.view_transcript {
            show_transcript(st, ui);
        } else {
            for c in list.iter().take(60) {
                let t0 = ops::src_to_out(&st.project.scene.slices, c.source_start_ms);
                let txt: String = c.text.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(34).collect();
                let label = format!("{}  {}", t0.map(fmt_time).unwrap_or_else(|| "—".into()), if txt.is_empty() { "(empty)".to_string() } else { txt });
                if note_row(ui, &label).clicked() {
                    st.select(Sel::Caption(c.id.clone()));
                    st.seek(t0.unwrap_or(0.0));
                }
            }
        }
    }

    kit::h3(ui, "Style", false);
    let presets: Vec<(String, &str)> = vec![("classic".into(), "Subtitle"), ("minimal".into(), "Minimal"), ("tiktok".into(), "TikTok")];
    seg(st, ui, Some("Preset"), &presets, sty.style.clone(), |p, v| preset(&v, &mut p.config.subtitles));
    let sty = st.project.config.subtitles.clone();
    slider(st, ui, "sub-size", "Size", sty.size, (24.0, 120.0, 1.0), px, |p, v| p.config.subtitles.size = v);
    slider(st, ui, "sub-y", "Height", sty.y, (0.15, 0.95, 0.01), pct, |p, v| p.config.subtitles.y = v);
    slider(st, ui, "sub-w", "Width", sty.width, (0.2, 0.95, 0.01), pct, |p, v| p.config.subtitles.width = v);
    color(st, ui, "sub-col", "Text color", &sty.color, |p, v| p.config.subtitles.color = v);
    toggle(st, ui, "Highlight spoken word", Some("Colours each word as it is said."), !sty.highlight.is_empty(), |p, v| p.config.subtitles.highlight = if v { "#ffe14a".into() } else { String::new() });
    if !sty.highlight.is_empty() {
        color(st, ui, "sub-hl", "Highlight color", &sty.highlight, |p, v| p.config.subtitles.highlight = v);
    }
    slider(st, ui, "sub-ow", "Stroke width", sty.outline_width, (0.0, 30.0, 1.0), px, |p, v| p.config.subtitles.outline_width = v);
    if sty.outline_width > 0.0 {
        color(st, ui, "sub-oc", "Stroke color", &sty.outline_color, |p, v| p.config.subtitles.outline_color = v);
    }
    toggle(st, ui, "Capital letters", None, sty.uppercase, |p, v| p.config.subtitles.uppercase = v);
    toggle(st, ui, "Pop in", Some("A quick bounce when each caption appears."), sty.pop, |p, v| p.config.subtitles.pop = v);
    toggle(st, ui, "Box behind text", None, !sty.background.is_empty(), |p, v| p.config.subtitles.background = if v { "#000000b3".into() } else { String::new() });
    let _ = srt::parse;
}

fn show_transcript(st: &mut EditorState, ui: &mut Ui) {
    let words = screencraft_core::transcript::extract_words(&st.project.scene.subtitles, &st.project.scene.slices);
    if words.is_empty() {
        return;
    }
    st.panel.caps.selected_words.retain(|&i| i < words.len());
    let fillers = screencraft_core::transcript::detect_fillers(&words);
    let pauses = screencraft_core::transcript::pause_gaps(&words, &st.project.scene.slices, 500.0);
    let pause_map: HashMap<usize, (f64, f64, bool)> = pauses.into_iter().map(|(i, a, b, cut)| (i, (a, b, cut))).collect();
    let cut_ms = screencraft_core::jumpcut::cut_total_ms(&st.project.scene.slices);

    kit::hint(ui, &format!("{} words · {:.1} s cut", words.len(), cut_ms / 1000.0));
    ui.add(
        egui::TextEdit::singleline(&mut st.panel.caps.search)
            .hint_text("Filter transcript…")
            .desired_width(f32::INFINITY)
            .margin(egui::Margin::symmetric(8, 5)),
    );
    ui.add_space(4.0);

    let sel_uncut: Vec<[f64; 2]> = st
        .panel
        .caps
        .selected_words
        .iter()
        .filter_map(|&i| words.get(i).filter(|w| !w.cut).map(|w| [w.start_ms, w.end_ms]))
        .collect();
    let sel_cut: Vec<[f64; 2]> = st
        .panel
        .caps
        .selected_words
        .iter()
        .filter_map(|&i| words.get(i).filter(|w| w.cut).map(|w| [w.start_ms, w.end_ms]))
        .collect();

    let mut cut_action: Option<Vec<[f64; 2]>> = None;
    let mut restore_action: Option<Vec<[f64; 2]>> = None;
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = Vec2::new(6.0, 4.0);
        if ui
            .add_enabled_ui(!sel_uncut.is_empty(), |ui| kit::pill(ui, None, "Cut selection", false, true, false, Some(Color32::from_rgb(0xff, 0x8a, 0x97))))
            .inner
            .on_hover_text("Create jump cuts for the selected transcript words (Del)")
            .clicked()
        {
            cut_action = Some(sel_uncut.clone());
        }
        if ui
            .add_enabled_ui(!sel_cut.is_empty(), |ui| kit::pill(ui, None, "Restore", false, true, false, None))
            .inner
            .on_hover_text("Restore the selected struck-through words")
            .clicked()
        {
            restore_action = Some(sel_cut.clone());
        }
        if !fillers.is_empty()
            && kit::pill(ui, None, &format!("Cut fillers ({})", fillers.len()), false, true, false, Some(CLIP))
                .on_hover_text("Cut all um/uh/er/hmm filler words from the video")
                .clicked()
        {
            cut_action = Some(fillers.iter().map(|&i| [words[i].start_ms, words[i].end_ms]).collect());
        }
    });
    ui.add_space(4.0);

    let query = st.panel.caps.search.trim().to_ascii_lowercase();
    let t_src = st.playhead_source_ms();
    let (shift, cmd) = ui.ctx().input(|i| (i.modifiers.shift, i.modifiers.command || i.modifiers.ctrl));
    let slices = st.project.scene.slices.clone();
    let mut seek_to: Option<f64> = None;

    egui::Frame::new()
        .fill(PANEL_2)
        .stroke(Stroke::new(1.0, LINE))
        .corner_radius(CornerRadius::same(8))
        .inner_margin(egui::Margin::same(8))
        .show(ui, |ui| {
            egui::ScrollArea::vertical().id_salt("transcript-scroll").max_height(230.0).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = Vec2::new(4.0, 5.0);
                    for (i, w) in words.iter().enumerate() {
                        if !query.is_empty() && !w.text.to_ascii_lowercase().contains(&query) {
                            continue;
                        }
                        let selected = st.panel.caps.selected_words.contains(&i);
                        let active = !w.cut && t_src >= w.start_ms && t_src <= w.end_ms;
                        let filler = !w.cut && screencraft_core::transcript::is_filler_word(&w.text);
                        let tw = kit::text_width(ui, &w.text, 12.0);
                        let (rect, resp) = ui.allocate_exact_size(Vec2::new(tw + 8.0, 22.0), Sense::click());
                        let hov = resp.hovered();
                        let bg = if selected {
                            ACCENT
                        } else if w.cut {
                            if hov { RAISE } else { BG }
                        } else if filler {
                            alpha(CLIP, if hov { 0.30 } else { 0.18 })
                        } else if hov {
                            RAISE
                        } else {
                            Color32::TRANSPARENT
                        };
                        if bg != Color32::TRANSPARENT {
                            ui.painter().rect_filled(rect, CornerRadius::same(4), bg);
                        }
                        if w.cut && !selected {
                            ui.painter().rect_stroke(rect, CornerRadius::same(4), Stroke::new(1.0, LINE), StrokeKind::Inside);
                        } else if active && !selected {
                            ui.painter().line_segment(
                                [Pos2::new(rect.min.x + 2.0, rect.max.y - 1.5), Pos2::new(rect.max.x - 2.0, rect.max.y - 1.5)],
                                Stroke::new(2.0, ACCENT_2),
                            );
                        }
                        let fg = if selected {
                            Color32::WHITE
                        } else if w.cut {
                            MUTED
                        } else if active {
                            Color32::WHITE
                        } else if filler {
                            Color32::from_rgb(0xf4, 0xb8, 0x86)
                        } else {
                            TEXT
                        };
                        ui.painter().text(rect.center(), Align2::CENTER_CENTER, &w.text, kit::font(12.0), fg);
                        if w.cut {
                            ui.painter().line_segment(
                                [Pos2::new(rect.min.x + 3.0, rect.center().y), Pos2::new(rect.max.x - 3.0, rect.center().y)],
                                Stroke::new(1.2, alpha(DANGER, 0.75)),
                            );
                        }
                        let tip = if w.cut {
                            "Cut from video · click to select, then Restore"
                        } else if filler {
                            "Filler word · click to jump, Del to cut"
                        } else {
                            "Click to jump · Shift-click range · Del to cut"
                        };
                        if resp.on_hover_text(tip).on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                            if shift {
                                let anchor = st.panel.caps.anchor_word.unwrap_or(i);
                                let (lo, hi) = (anchor.min(i), anchor.max(i));
                                for idx in lo..=hi {
                                    st.panel.caps.selected_words.insert(idx);
                                }
                            } else if cmd {
                                if !st.panel.caps.selected_words.remove(&i) {
                                    st.panel.caps.selected_words.insert(i);
                                }
                                st.panel.caps.anchor_word = Some(i);
                            } else {
                                st.panel.caps.selected_words.clear();
                                st.panel.caps.selected_words.insert(i);
                                st.panel.caps.anchor_word = Some(i);
                            }
                            if !w.cut {
                                if let Some(t_out) = ops::src_to_out(&slices, w.start_ms) {
                                    seek_to = Some(t_out);
                                }
                            }
                        }

                        if query.is_empty() {
                            if let Some(&(pa, pb, pcut)) = pause_map.get(&i) {
                                let label = format!("· {:.1}s ·", (pb - pa + 60.0) / 1000.0);
                                let pw = kit::text_width(ui, &label, 10.5) + 8.0;
                                let (pr, presp) = ui.allocate_exact_size(Vec2::new(pw, 20.0), Sense::click());
                                let pbg = if presp.hovered() { RAISE } else { alpha(LINE, 0.6) };
                                ui.painter().rect_filled(pr, CornerRadius::same(4), pbg);
                                ui.painter().text(pr.center(), Align2::CENTER_CENTER, &label, kit::font(10.5), MUTED);
                                if pcut {
                                    ui.painter().line_segment(
                                        [Pos2::new(pr.min.x + 2.0, pr.center().y), Pos2::new(pr.max.x - 2.0, pr.center().y)],
                                        Stroke::new(1.0, alpha(DANGER, 0.7)),
                                    );
                                }
                                let ptip = if pcut { "Cut pause · click to restore" } else { "Pause · click to cut from video" };
                                if presp.on_hover_text(ptip).on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                                    if pcut {
                                        restore_action = Some(vec![[pa, pb]]);
                                    } else {
                                        cut_action = Some(vec![[pa, pb]]);
                                    }
                                }
                            }
                        }
                    }
                });
            });
        });

    if let Some(t) = seek_to {
        st.pause();
        st.seek(t);
    }
    if let Some(ranges) = cut_action {
        st.cut_source_ranges(&ranges);
        st.panel.caps.selected_words.clear();
        st.panel.caps.anchor_word = None;
    } else if let Some(ranges) = restore_action {
        st.restore_source_ranges(&ranges);
        st.panel.caps.selected_words.clear();
        st.panel.caps.anchor_word = None;
    }
    kit::hint(ui, "Click words to jump, Shift-click a range, and press Del or Cut selection to create jump cuts. Click struck-through words to restore.");
}
