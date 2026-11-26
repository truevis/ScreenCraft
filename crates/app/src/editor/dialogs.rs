//! Popover menus (Presets, Raw files) and the About & privacy window.

use super::{open_url, EditorState};
use crate::kit;
use crate::theme::*;
use eframe::egui::{self, Align2, Color32, CornerRadius, Pos2, Rect, Sense, Stroke, StrokeKind, Vec2};
use screencraft_core::presets::{apply_preset_value, builtin_presets};
use serde_json::{json, Value};
use std::collections::BTreeMap;

pub struct Dialogs {
    pub about: bool,
    pub discard: bool,
    pub raw: bool,
    pub projects: bool,
    pub projects_anchor: Option<Rect>,
    pub raw_anchor: Option<Rect>,
    pub presets_open: bool,
    pub presets_manage: bool,
    pub presets_anchor: Option<Rect>,
    rename_buf: BTreeMap<String, String>,
    focus_name: Option<String>,
    cached_projects: Option<(std::path::PathBuf, Vec<(std::path::PathBuf, String)>)>,
    cached_presets: Option<Vec<(String, Value)>>,
}

impl Default for Dialogs {
    fn default() -> Self {
        Self {
            about: false,
            discard: false,
            raw: false,
            projects: false,
            projects_anchor: None,
            raw_anchor: None,
            presets_open: false,
            presets_manage: false,
            presets_anchor: None,
            rename_buf: BTreeMap::new(),
            focus_name: None,
            cached_projects: None,
            cached_presets: None,
        }
    }
}

// ---- presets -----------------------------------------------------------------------------

fn presets_path() -> Option<std::path::PathBuf> {
    dirs::config_dir().map(|d| d.join("ScreenCraft").join("presets.json"))
}

fn user_presets() -> Vec<(String, Value)> {
    presets_path()
        .and_then(|p| std::fs::read(p).ok())
        .and_then(|b| serde_json::from_slice::<serde_json::Map<String, Value>>(&b).ok())
        .map(|m| m.into_iter().collect())
        .unwrap_or_default()
}

fn save_user_presets(all: &[(String, Value)]) {
    if let Some(p) = presets_path() {
        if let Some(d) = p.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        let m: serde_json::Map<String, Value> = all.iter().cloned().collect();
        let tmp = p.with_extension("json.tmp");
        if let Err(e) = std::fs::write(&tmp, serde_json::to_vec_pretty(&m).unwrap_or_default()).and_then(|_| std::fs::rename(&tmp, &p)) {
            crate::applog::line(&format!("could not save presets: {e}"));
        }
    }
}

// ---- popover chrome -----------------------------------------------------------------------

struct Menu<'a> {
    ui: &'a mut egui::Ui,
}

impl Menu<'_> {
    fn header(&mut self, text: &str) {
        let (r, _) = self.ui.allocate_exact_size(Vec2::new(self.ui.available_width(), 22.0), Sense::hover());
        kit::spaced_text(self.ui, Pos2::new(r.min.x + 10.0, r.min.y + 6.0), &text.to_uppercase(), 10.0, MUTED, 1.0);
    }
    fn hr(&mut self) {
        self.ui.add_space(5.0);
        let (r, _) = self.ui.allocate_exact_size(Vec2::new(self.ui.available_width(), 1.0), Sense::hover());
        self.ui.painter().rect_filled(r, 0.0, LINE_2);
        self.ui.add_space(5.0);
    }
    fn item(&mut self, label: &str) -> bool {
        let (r, resp) = self.ui.allocate_exact_size(Vec2::new(self.ui.available_width(), 30.0), Sense::click());
        if resp.hovered() {
            self.ui.painter().rect_filled(r, CornerRadius::same(6), RAISE);
        }
        self.ui.painter().text(Pos2::new(r.min.x + 10.0, r.center().y), Align2::LEFT_CENTER, label, kit::font(13.0), TEXT);
        resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked()
    }
}

/// Show `content` in a popover under `anchor`. Returns true when a click landed outside of it.
fn popover(ctx: &egui::Context, id: &str, anchor: Rect, align_left: bool, min_w: f32, max_w: f32, content: impl FnOnce(&mut Menu)) -> bool {
    let mut outside = false;
    let x = if align_left { anchor.left() } else { anchor.right() - 230.0 };
    let area = egui::Area::new(egui::Id::new(id)).order(egui::Order::Foreground).fixed_pos(Pos2::new(x, anchor.bottom() + 6.0)).show(ctx, |ui| {
        egui::Frame::new()
            .fill(PANEL_2)
            .stroke(Stroke::new(1.0, LINE_2))
            .corner_radius(CornerRadius::same(10))
            .inner_margin(egui::Margin::same(6))
            .shadow(egui::Shadow { offset: [0, 14], blur: 44, spread: 0, color: Color32::from_black_alpha(140) })
            .show(ui, |ui| {
                ui.set_min_width(min_w);
                ui.set_max_width(max_w);
                ui.spacing_mut().item_spacing = Vec2::ZERO;
                content(&mut Menu { ui });
            });
    });
    if ctx.input(|i| i.pointer.any_pressed()) {
        if let Some(p) = ctx.input(|i| i.pointer.interact_pos()) {
            if !area.response.rect.contains(p) && !anchor.contains(p) {
                outside = true;
            }
        }
    }
    outside
}

pub fn show(st: &mut EditorState, ctx: &egui::Context, prefs: &crate::prefs::Prefs) {
    projects_menu(st, ctx, prefs);
    presets_menu(st, ctx);
    raw_menu(st, ctx);
    about(st, ctx);
}

fn projects_menu(st: &mut EditorState, ctx: &egui::Context, prefs: &crate::prefs::Prefs) {
    if !st.dialogs.projects {
        st.dialogs.cached_projects = None;
        return;
    }
    let anchor = st.dialogs.projects_anchor.unwrap_or(Rect::from_min_size(Pos2::new(60.0, 40.0), Vec2::new(90.0, 32.0)));
    let dir = st.package.dir.clone();
    let root = prefs.recordings_dir.clone();
    if st.dialogs.cached_projects.as_ref().map(|(r, _)| r != &root).unwrap_or(true) {
        let pkgs = screencraft_engine::package::list_packages(&root)
            .into_iter()
            .filter(|(p, _)| p.dir != dir)
            .take(8)
            .map(|(pkg, _)| {
                let name = pkg.load_project().map(|p| p.name).unwrap_or_else(|_| pkg.dir.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default());
                (pkg.dir, name)
            })
            .collect();
        st.dialogs.cached_projects = Some((root.clone(), pkgs));
    }
    let list = st.dialogs.cached_projects.as_ref().map(|(_, v)| v.as_slice()).unwrap_or(&[]);
    let mut open: Option<std::path::PathBuf> = None;
    let mut reveal_it = false;
    let mut reveal_root = false;
    let mut footage: Option<super::FootageRequest> = None;
    let (mut pick_open, mut pick_add) = (false, false);
    let default_idx = st.default_insert_index();
    let outside = popover(ctx, "projects-menu", anchor, true, 260.0, 320.0, |m| {
        m.header("Record");
        if m.item("New recording") {
            footage = Some(super::FootageRequest::NewRecording);
        }
        if m.item("Record more for this project") {
            footage = Some(super::FootageRequest::RecordMore { insert_at: default_idx });
        }
        if m.item("Add a recording or video to this project…") {
            pick_add = true;
        }
        m.hr();
        m.header("Recent Projects");
        for (pdir, name) in list {
            if m.item(name) {
                open = Some(pdir.clone());
            }
        }
        if list.is_empty() {
            m.item("No other projects yet");
        }
        if m.item("Open project…") {
            pick_open = true;
        }
        m.hr();
        if m.item("Show this project in the file manager") {
            reveal_it = true;
        }
        if m.item("Open recordings folder") {
            reveal_root = true;
        }
    });
    if let Some(f) = footage {
        st.dialogs.projects = false;
        st.footage_request = Some(f);
    } else if pick_open {
        st.dialogs.projects = false;
        if let Some(d) = pick_project(&root) {
            st.open_request = Some(d);
        }
    } else if pick_add {
        st.dialogs.projects = false;
        if let Some(p) = pick_footage(&root) {
            st.footage_request = Some(super::FootageRequest::Add { path: p, insert_at: default_idx });
        }
    } else if let Some(d) = open {
        st.dialogs.projects = false;
        st.open_request = Some(d);
    } else if reveal_it {
        st.dialogs.projects = false;
        reveal(&dir.join(&st.project.media.screen));
    } else if reveal_root {
        st.dialogs.projects = false;
        let _ = open_url(&root.to_string_lossy());
    } else if outside {
        st.dialogs.projects = false;
    }
}

fn presets_menu(st: &mut EditorState, ctx: &egui::Context) {
    if !st.dialogs.presets_open {
        st.dialogs.cached_presets = None;
        return;
    }
    let anchor = st.dialogs.presets_anchor.unwrap_or(Rect::from_min_size(Pos2::new(900.0, 40.0), Vec2::new(100.0, 32.0)));
    let manage = st.dialogs.presets_manage;
    let mut apply: Option<Value> = None;
    let mut close = false;
    let mut save_new = false;
    let mut toggle_manage = false;
    let mut delete: Option<String> = None;
    let mut rename: Option<(String, String)> = None;
    if st.dialogs.cached_presets.is_none() {
        st.dialogs.cached_presets = Some(user_presets());
    }
    let mut user = st.dialogs.cached_presets.take().unwrap_or_default();
    let focus = st.dialogs.focus_name.clone();
    let mut bufs = std::mem::take(&mut st.dialogs.rename_buf);
    let outside = popover(ctx, "presets-menu", anchor, false, 210.0, 260.0, |m| {
        if !manage {
            for (n, v) in builtin_presets() {
                if m.item(n) {
                    apply = Some(v);
                    close = true;
                }
            }
            if !user.is_empty() {
                m.hr();
                for (n, v) in &user {
                    if m.item(n) {
                        apply = Some(v.clone());
                        close = true;
                    }
                }
            }
            m.hr();
            // Save Preset + gear
            let (r, _) = m.ui.allocate_exact_size(Vec2::new(m.ui.available_width(), 32.0), Sense::hover());
            let save = Rect::from_min_max(r.min, Pos2::new(r.max.x - 40.0, r.max.y));
            let gear = Rect::from_min_max(Pos2::new(r.max.x - 34.0, r.min.y), r.max);
            let rs = m.ui.interact(save, m.ui.id().with("save"), Sense::click());
            m.ui.painter().rect_filled(save, CornerRadius::same(6), if rs.hovered() { LINE } else { RAISE });
            m.ui.painter().text(save.center(), Align2::CENTER_CENTER, "Save Preset", kit::font(13.0), TEXT);
            if rs.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                save_new = true;
            }
            let rg = m.ui.interact(gear, m.ui.id().with("gear"), Sense::click());
            m.ui.painter().rect_stroke(gear, CornerRadius::same(6), Stroke::new(1.0, if rg.hovered() { ACCENT_2 } else { LINE_2 }), StrokeKind::Inside);
            crate::icons::paint(m.ui, "gear", Rect::from_center_size(gear.center(), Vec2::splat(16.0)), ICON);
            if rg.on_hover_text("Rename or delete your presets").on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                toggle_manage = true;
            }
        } else {
            m.header("Your presets");
            if user.is_empty() {
                m.ui.add_space(4.0);
                m.ui.label(egui::RichText::new("No saved presets yet. Use Save Preset first.").size(12.0).color(MUTED));
                m.ui.add_space(4.0);
            }
            for (n, _) in &user {
                let buf = bufs.entry(n.clone()).or_insert_with(|| n.clone());
                m.ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;
                    let r = ui.add_sized([150.0, 28.0], egui::TextEdit::singleline(buf).margin(egui::Margin::symmetric(8, 6)).char_limit(40));
                    if focus.as_deref() == Some(n.as_str()) {
                        r.request_focus();
                    }
                    if r.lost_focus() {
                        let next = buf.trim().to_string();
                        if !next.is_empty() && next != *n {
                            rename = Some((n.clone(), next));
                        } else {
                            *buf = n.clone();
                        }
                    }
                    if kit::icon_btn_sized(ui, "trash", "Delete preset", 28.0, 15.0, true).clicked() {
                        delete = Some(n.clone());
                    }
                });
            }
            m.hr();
            let (r, resp) = m.ui.allocate_exact_size(Vec2::new(m.ui.available_width(), 32.0), Sense::click());
            m.ui.painter().rect_filled(r, CornerRadius::same(6), if resp.hovered() { LINE } else { RAISE });
            m.ui.painter().text(r.center(), Align2::CENTER_CENTER, "Done", kit::font(13.0), TEXT);
            if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                toggle_manage = true;
            }
        }
    });
    st.dialogs.rename_buf = bufs;
    st.dialogs.focus_name = None;
    if let Some(v) = apply {
        st.edit(|p| apply_preset_value(p, &v));
    }
    if save_new {
        let mut n = user.len() + 1;
        while user.iter().any(|(k, _)| k == &format!("My preset {n}")) {
            n += 1;
        }
        let name = format!("My preset {n}");
        let c = &st.project.config;
        user.push((name.clone(), json!({ "styles": c.styles, "cursor": c.cursor, "camera": c.camera })));
        save_user_presets(&user);
        st.dialogs.presets_manage = true;
        st.dialogs.focus_name = Some(name);
    }
    if let Some(n) = delete {
        user.retain(|(k, _)| k != &n);
        save_user_presets(&user);
        st.dialogs.rename_buf.remove(&n);
    }
    if let Some((from, to)) = rename {
        if !user.iter().any(|(k, _)| k == &to) {
            for (k, _) in user.iter_mut() {
                if *k == from {
                    *k = to.clone();
                }
            }
            save_user_presets(&user);
        }
        st.dialogs.rename_buf.remove(&from);
    }
    if toggle_manage {
        st.dialogs.presets_manage = !st.dialogs.presets_manage;
    }
    if close || outside {
        st.dialogs.presets_open = false;
        st.dialogs.presets_manage = false;
        st.dialogs.presets_anchor = None;
        st.dialogs.cached_presets = None;
    } else {
        st.dialogs.cached_presets = Some(user);
    }
}

fn raw_menu(st: &mut EditorState, ctx: &egui::Context) {
    if !st.dialogs.raw {
        return;
    }
    let anchor = st.dialogs.raw_anchor.unwrap_or(Rect::from_min_size(Pos2::new(820.0, 40.0), Vec2::new(60.0, 32.0)));
    let media = st.project.media.clone();
    let dir = st.package.dir.clone();
    let name = st.project.name.clone();
    let mut action: Option<u8> = None;
    let outside = popover(ctx, "raw-menu", anchor, false, 210.0, 260.0, |m| {
        m.header("Raw recording files");
        let primary_label = if media.camera_only { "Save raw camera video as…" } else { "Save raw screen video as…" };
        if m.item(primary_label) {
            action = Some(0);
        }
        if media.camera.is_some() && m.item("Save raw camera video as…") {
            action = Some(1);
        }
        m.hr();
        if m.item("Open raw video in compressor…") {
            action = Some(3);
        }
        let fm = if cfg!(target_os = "macos") { "Finder" } else if cfg!(target_os = "windows") { "Explorer" } else { "the file manager" };
        if m.item(&format!("Show raw files in {fm}")) {
            action = Some(2);
        }
    });
    if let Some(a) = action {
        st.dialogs.raw = false;
        match a {
            0 | 1 => {
                let (rel, label) = if a == 0 {
                    (media.screen.clone(), if media.camera_only { "(raw camera)" } else { "(raw screen)" })
                } else {
                    (media.camera.clone().unwrap_or_default(), "(raw camera)")
                };
                let ext = std::path::Path::new(&rel).extension().and_then(|e| e.to_str()).unwrap_or("mp4").to_string();
                let base = name.replace(['/', '\\', ':'], "-");
                if let Some(dst) = rfd::FileDialog::new().set_file_name(format!("{base} {label}.{ext}")).add_filter(ext.to_uppercase(), &[&ext]).save_file() {
                    match std::fs::copy(dir.join(&rel), &dst) {
                        Ok(_) => reveal(&dst),
                        Err(e) => st.error = Some(format!("Could not copy: {e}")),
                    }
                }
            }
            3 => st.compress_request = Some(dir.join(&media.screen)),
            _ => reveal(&dir.join(&media.screen)),
        }
    } else if outside {
        st.dialogs.raw = false;
    }
}

/// Ask for a project: its `project.json` (or any file in its `.screencraft` folder). Returns the folder.
pub fn pick_project(root: &std::path::Path) -> Option<std::path::PathBuf> {
    let f = rfd::FileDialog::new().set_title("Open a ScreenCraft project (pick project.json inside its .screencraft folder)").set_directory(root).add_filter("ScreenCraft project", &["json"]).pick_file()?;
    project_dir_of(&f)
}

/// The `.screencraft` folder a picked file belongs to, if any.
pub fn project_dir_of(f: &std::path::Path) -> Option<std::path::PathBuf> {
    let d = if f.is_dir() { f.to_path_buf() } else { f.parent()?.to_path_buf() };
    d.join("project.json").exists().then_some(d)
}

/// Ask for footage to add: an MP4, or a file of another recording (its project.json or screen video).
pub fn pick_footage(root: &std::path::Path) -> Option<std::path::PathBuf> {
    let f = rfd::FileDialog::new()
        .set_title("Add a video, or another recording's project.json")
        .set_directory(root)
        .add_filter("Video or ScreenCraft project", &["mp4", "m4v", "mov", "json"])
        .pick_file()?;
    Some(project_dir_of(&f).unwrap_or(f))
}

pub fn reveal(path: &std::path::Path) {
    #[cfg(target_os = "macos")]
    let _ = std::process::Command::new("open").arg("-R").arg(path).spawn();
    #[cfg(target_os = "windows")]
    {
        // Explorer parses its own command line: `.arg()` would quote the whole `/select,...`
        // token when the path has spaces, which Explorer rejects and opens Documents instead.
        // Pass `/select,"C:\full\path"` verbatim, with backslashes and an absolute path.
        use std::os::windows::process::CommandExt;
        let abs = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        let s = abs.display().to_string();
        let s = s.strip_prefix(r"\\?\").unwrap_or(&s).replace('/', "\\");
        let _ = std::process::Command::new("explorer").raw_arg(format!("/select,\"{s}\"")).spawn();
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    let _ = std::process::Command::new("xdg-open").arg(path.parent().unwrap_or(path)).spawn();
}

fn card(ui: &mut egui::Ui, tint: bool, add: impl FnOnce(&mut egui::Ui)) {
    let (fill, border) = if tint { (alpha(ACCENT, 0.12), alpha(Color32::from_rgb(106, 174, 232), 0.55)) } else { (PANEL, LINE) };
    egui::Frame::new().fill(fill).stroke(Stroke::new(1.0, border)).corner_radius(CornerRadius::same(10)).inner_margin(egui::Margin::symmetric(16, 14)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        add(ui);
    });
}

fn h2(ui: &mut egui::Ui, text: &str) {
    let (r, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 14.0), Sense::hover());
    kit::spaced_text(ui, r.min, &text.to_uppercase(), 10.5, MUTED, 1.26);
    ui.add_space(8.0 - ui.spacing().item_spacing.y);
}

fn para(ui: &mut egui::Ui, text: &str) {
    ui.add(egui::Label::new(egui::RichText::new(text).size(13.0).color(LABEL)).wrap());
}

fn about(st: &mut EditorState, ctx: &egui::Context) {
    if !st.dialogs.about {
        return;
    }
    let mut open = true;
    if kit::modal(ctx, "About ScreenCraft", 520.0, BG, egui::Margin::symmetric(30, 26), 168, |ui| {
        ui.spacing_mut().item_spacing = Vec2::new(8.0, 16.0);
        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 2.0;
                ui.label(egui::RichText::new("ScreenCraft").size(22.0).color(TEXT));
                ui.label(egui::RichText::new(format!("Version {}", env!("CARGO_PKG_VERSION"))).size(12.0).color(MUTED));
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::TOP), |ui| {
                if kit::icon_btn(ui, "close", "Close").clicked() {
                    open = false;
                }
            });
        });
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 6.0;
            h2(ui, "Privacy");
            card(ui, false, |ui| {
                ui.spacing_mut().item_spacing.y = 8.0;
                ui.add(egui::Label::new(egui::RichText::new("ScreenCraft works on your computer. Your recordings, screen, camera, microphone, cursor data and captions are never uploaded. There are no accounts, no analytics, no telemetry and no update checks.").size(13.0).color(LABEL)).wrap());
                para(ui, "One thing uses the network, and only when you ask:");
                para(ui, "•  Caption models. Only if you ask to generate captions, a speech model file is downloaded once from Hugging Face. Your audio is never sent anywhere; recognition runs on your machine.");
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing.x = 14.0;
                    for (t, u) in [
                        ("Third-party notices", "https://github.com/truevis/ScreenCraft/blob/ci/windows-installer/THIRD-PARTY-NOTICES.md"),
                        ("Source code (MIT)", "https://github.com/truevis/ScreenCraft"),
                        ("Report a bug", "https://github.com/truevis/ScreenCraft/issues"),
                        ("X (@mdnlabs)", "https://x.com/mdnlabs"),
                    ] {
                        if ui.add(egui::Label::new(egui::RichText::new(t).size(12.5).color(ACCENT_2)).sense(Sense::click())).on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                            let _ = open_url(u);
                        }
                    }
                });
            });
        });
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 6.0;
            h2(ui, "Keyboard shortcuts");
            card(ui, false, |ui| {
                ui.spacing_mut().item_spacing.y = 4.0;
                for (keys, desc) in [
                    ("Space (tap / hold)", "Play / pause (hold for 2× speed)"),
                    ("← / → (Shift)", "Step 1 frame / 1 s (or nudge selected item)"),
                    ("Q / E  ·  A / D", "Start / end  ·  Previous / next zoom"),
                    ("S / X / Z", "Split at playhead / toggle jump cut / add zoom"),
                    ("Ctrl+D / Del", "Duplicate / delete selected item"),
                    ("Ctrl+Z / Ctrl+Y", "Undo / redo"),
                    ("Ctrl+S / Ctrl+E / F11", "Save / export / full preview"),
                ] {
                    ui.horizontal(|ui| {
                        ui.allocate_ui(Vec2::new(155.0, 16.0), |ui| {
                            ui.label(egui::RichText::new(keys).size(12.0).strong().color(TEXT));
                        });
                        ui.label(egui::RichText::new(desc).size(12.0).color(MUTED));
                    });
                }
            });
        });
    }) {
        open = false;
    }
    if !open || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        st.dialogs.about = false;
    }
}

/// "Save changes to ...?" over the editor, shown before the window closes or another project opens.
pub fn save_prompt(st: &mut EditorState, ctx: &egui::Context) {
    if st.close_prompt.is_none() {
        return;
    }
    let mut answer = None;
    let _ = kit::modal(ctx, "save-ask", 350.0, PANEL, egui::Margin::same(20), 158, |ui| {
        ui.spacing_mut().item_spacing = Vec2::new(8.0, 6.0);
        ui.label(egui::RichText::new(format!("Save changes to “{}”?", st.project.name)).strong().size(15.0).color(TEXT));
        ui.label(egui::RichText::new("Your edits will be lost if you don't save them.").size(12.5).color(MUTED));
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            if kit::pill(ui, None, "Don't Save", false, false, false, None).clicked() {
                answer = Some(super::Answer::Discard);
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if kit::primary(ui, None, "Save").clicked() {
                    answer = Some(super::Answer::Save);
                }
                if kit::pill(ui, None, "Cancel", false, false, false, None).clicked() {
                    answer = Some(super::Answer::Cancel);
                }
            });
        });
    });
    if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        answer = Some(super::Answer::Cancel);
    }
    if let Some(a) = answer {
        st.close_answer = Some(a);
    }
}

/// Confirmation modal for permanently discarding the current recording/project.
pub fn discard_prompt(st: &mut EditorState, ctx: &egui::Context) {
    if !st.dialogs.discard {
        return;
    }
    let mut confirm = false;
    let mut cancel = false;
    let outside = kit::modal(ctx, "discard-ask", 370.0, PANEL, egui::Margin::same(20), 158, |ui| {
        ui.spacing_mut().item_spacing = Vec2::new(8.0, 6.0);
        ui.label(egui::RichText::new(format!("Discard “{}”?", st.project.name)).strong().size(15.0).color(TEXT));
        ui.add(
            egui::Label::new(
                egui::RichText::new("This permanently deletes this recording and its project files. This can't be undone.")
                    .size(12.5)
                    .color(MUTED),
            )
            .wrap(),
        );
        ui.add_space(10.0);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if kit::danger(ui, Some("trash"), "Discard").clicked() {
                confirm = true;
            }
            if kit::pill(ui, None, "Cancel", false, false, false, None).clicked() {
                cancel = true;
            }
        });
    });
    if outside || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        cancel = true;
    }
    if confirm {
        st.dialogs.discard = false;
        st.discard_request = true;
    } else if cancel {
        st.dialogs.discard = false;
    }
}
