//! Application shell: the recorder bar is the root window (bottom-centre, always on top); every
//! open project gets its own editor window, like the 0.3 app.

use crate::editor::EditorState;
use crate::prefs::Prefs;
use crate::recorder::{Action, Evt, Mode, Phase, Recorder, BAR_H};
use eframe::egui::{self, Color32, Pos2, Vec2, ViewportBuilder, ViewportCommand, ViewportId};
use global_hotkey::hotkey::{Code, HotKey, Modifiers};
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
use screencraft_core::project::Rect as PRect;
use screencraft_engine::Package;
use std::sync::Arc;
use std::time::{Duration, Instant};

struct EditorWin {
    id: ViewportId,
    ed: Box<EditorState>,
    title: String,
}

pub struct App {
    prefs: Prefs,
    rec: Recorder,
    editors: Vec<EditorWin>,
    next_editor: u64,
    render_state: eframe::egui_wgpu::RenderState,
    _hotkeys: Option<GlobalHotKeyManager>,
    hotkey_id: Option<u32>,
    hotkey_enabled: bool,
    /// Developer hooks: open a package, seek and save a screenshot, then quit.
    shot: Option<Shot>,
    /// `SCREENCRAFT_AUTORECORD=<seconds>`: record the main display, stop, report, quit.
    autorecord: Option<f64>,
    autorecord_started: bool,
    /// Without a window (screenshots and tests): the editor draws inline.
    headless: bool,
    /// The window size last asked for, so the bar is only moved when something changed.
    applied: Option<(Vec2, Phase, bool)>,
    area_picker: Option<crate::area::Picker>,
    /// Highlight outline on the screen while hovering a window in the menu.
    hover_window: Option<screencraft_capture::WindowInfo>,
    compressor: Option<crate::compressor::Compressor>,
    pending_open: Vec<std::path::PathBuf>,
    /// Recordings just finished: their zooms are generated the first time they open.
    fresh: Vec<std::path::PathBuf>,
    quit: bool,
    quit_pending: bool,
    discard_after_stop: bool,
    /// Native window handle (Windows), to keep the recording bar out of the capture.
    native_window: Option<isize>,
    /// "Record more for this project": the next recording is added to the end of this project.
    append_target: Option<(std::path::PathBuf, String)>,
    /// Footage being added to a project (runs on its own thread).
    append_job: Option<AppendJob>,
    /// Floating camera viewfinder dismissed by user until re-selecting Camera mode.
    cam_preview_hidden: bool,
    cam_preview_anchor: Option<Pos2>,
}

/// Joining a recording or video onto the end of a project, off the UI thread.
struct AppendJob {
    target: std::path::PathBuf,
    /// A recording made just for this (removed once it is in the project).
    temp: Option<std::path::PathBuf>,
    progress: Arc<std::sync::atomic::AtomicU32>,
    cancel: Arc<std::sync::atomic::AtomicBool>,
    result: Arc<std::sync::Mutex<Option<Result<screencraft_engine::append::Appended, String>>>>,
}

impl AppendJob {
    fn start(target: std::path::PathBuf, extra: std::path::PathBuf, temp: bool) -> Self {
        use std::sync::atomic::Ordering;
        let progress = Arc::new(std::sync::atomic::AtomicU32::new(0));
        let cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let result = Arc::new(std::sync::Mutex::new(None));
        let (p, c, r, t, e) = (progress.clone(), cancel.clone(), result.clone(), target.clone(), extra.clone());
        std::thread::spawn(move || {
            let res = screencraft_engine::append::append(&t, &e, |f| {
                p.store((f * 1000.0) as u32, Ordering::Relaxed);
                !c.load(Ordering::Relaxed)
            });
            *r.lock().unwrap() = Some(res);
        });
        Self { target, temp: temp.then_some(extra), progress, cancel, result }
    }
}

struct Shot {
    path: std::path::PathBuf,
    after_frames: u32,
    frame: u32,
    requested: bool,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        crate::theme::install(&cc.egui_ctx);
        let render_state = cc.wgpu_render_state.clone().expect("the wgpu renderer is required");
        Self::build(&cc.egui_ctx, render_state, true)
    }

    /// Without a window or global shortcuts: for the screenshot tool and tests.
    pub fn new_headless(ctx: &egui::Context, render_state: eframe::egui_wgpu::RenderState) -> Self {
        let mut a = Self::build(ctx, render_state, false);
        a.headless = true;
        a
    }

    pub fn open_package(&mut self, ctx: &egui::Context, dir: std::path::PathBuf) {
        // already open: nothing to do
        if self.editors.iter().any(|e| e.ed.package.dir == dir) {
            return;
        }
        match EditorState::open(Package::open(dir), ctx, self.render_state.clone()) {
            Ok(mut ed) => {
                if let Some(i) = self.fresh.iter().position(|d| *d == ed.package.dir) {
                    self.fresh.remove(i);
                    if ed.project.media.camera_only {
                        // a talking-head video opens with its pauses already switched off (they
                        // stay on the timeline, greyed out, to adjust or turn back on)
                        if ed.project.scene.slices.iter().all(|s| !s.disabled) {
                            let _ = ed.auto_jump_cuts(Default::default());
                            ed.save_now();
                        }
                    } else if ed.project.scene.zooms.is_empty() {
                        ed.auto_zoom();
                        ed.save_now();
                    }
                }
                self.next_editor += 1;
                let title = ed.project.name.clone();
                self.editors.push(EditorWin { id: ViewportId::from_hash_of(("editor", self.next_editor)), ed: Box::new(ed), title });
            }
            Err(e) => self.rec.show_error(e),
        }
    }

    /// Show the compact recording bar as if a recording were running (screenshots only).
    pub fn dev_bar(&mut self) {
        self.rec.phase = Phase::Idle;
    }

    pub fn dev_hud(&mut self) {
        self.rec.phase = Phase::Recording;
        self.rec.started_at = Some(Instant::now() - Duration::from_secs(83));
    }

    pub fn dev_menu(&mut self, which: &str) {
        let items = match which {
            "quality" => self.rec.menu_for_dev(crate::recorder::MenuKind::Quality, &self.prefs),
            "sys" => self.rec.menu_for_dev(crate::recorder::MenuKind::Sys, &self.prefs),
            "folder" => self.rec.menu_for_dev(crate::recorder::MenuKind::Folder, &self.prefs),
            "camera" => self.rec.menu_for_dev(crate::recorder::MenuKind::Camera, &self.prefs),
            "mode-camera" => {
                self.rec.mode = Mode::Camera;
                self.prefs.vertical = true;
                return;
            }
            _ => return,
        };
        self.rec.menu = Some(items);
    }

    pub fn dev_seek(&mut self, ms: f64) {
        if let Some(e) = self.editors.first_mut() {
            e.ed.seek(ms);
        }
    }

    pub fn dev_select(&mut self, what: Option<&str>) {
        if let Some(e) = self.editors.first_mut() {
            e.ed.apply_dev_selection(what);
        }
    }

    fn sync_hotkey(&mut self) {
        if self.hotkey_enabled == self.prefs.global_hotkey {
            return;
        }
        self.hotkey_enabled = self.prefs.global_hotkey;
        let mods = if cfg!(target_os = "macos") { Modifiers::SUPER | Modifiers::SHIFT } else { Modifiers::CONTROL | Modifiers::SHIFT };
        let hk = HotKey::new(Some(mods), Code::KeyR);
        if let Some(mgr) = &self._hotkeys {
            if self.prefs.global_hotkey {
                let id = hk.id();
                if mgr.register(hk).is_ok() {
                    self.hotkey_id = Some(id);
                }
            } else {
                let _ = mgr.unregister(hk);
                self.hotkey_id = None;
            }
        }
    }

    fn build(ctx: &egui::Context, render_state: eframe::egui_wgpu::RenderState, hotkeys: bool) -> Self {
        let mut prefs = Prefs::load();
        if std::env::var("SCREENCRAFT_AUTORECORD_MODE").as_deref() == Ok("camera") {
            prefs.mode = "camera".into();
            prefs.camera = true;
        }
        let (mgr, id) = match if hotkeys { GlobalHotKeyManager::new() } else { Err(global_hotkey::Error::AlreadyRegistered(HotKey::new(None, Code::KeyA))) } {
            Ok(m) => {
                let id = if prefs.global_hotkey {
                    let mods = if cfg!(target_os = "macos") { Modifiers::SUPER | Modifiers::SHIFT } else { Modifiers::CONTROL | Modifiers::SHIFT };
                    let hk = HotKey::new(Some(mods), Code::KeyR);
                    let id = hk.id();
                    m.register(hk).is_ok().then_some(id)
                } else {
                    None
                };
                (Some(m), id)
            }
            Err(_) => (None, None),
        };
        let mut rec = Recorder::new(ctx, &prefs);
        rec.permissions = screencraft_capture::permissions();
        let hotkey_enabled = prefs.global_hotkey;
        let mut app = Self {
            rec,
            prefs,
            editors: Vec::new(),
            next_editor: 0,
            render_state,
            _hotkeys: mgr,
            hotkey_id: id,
            hotkey_enabled,
            autorecord: std::env::var("SCREENCRAFT_AUTORECORD").ok().and_then(|v| v.parse().ok()),
            autorecord_started: false,
            headless: false,
            applied: None,
            area_picker: None,
            hover_window: None,
            compressor: None,
            pending_open: Vec::new(),
            append_target: None,
            append_job: None,
            cam_preview_hidden: false,
            cam_preview_anchor: None,
            fresh: Vec::new(),
            quit: false,
            quit_pending: false,
            discard_after_stop: false,
            native_window: None,
            shot: std::env::var_os("SCREENCRAFT_SHOT").map(|p| Shot {
                path: p.into(),
                after_frames: std::env::var("SCREENCRAFT_SHOT_FRAMES").ok().and_then(|v| v.parse().ok()).unwrap_or(40),
                frame: 0,
                requested: false,
            }),
        };
        if let Some(dir) = std::env::var_os("SCREENCRAFT_OPEN") {
            app.open_package(ctx, dir.into());
            if let Ok(ms) = std::env::var("SCREENCRAFT_SEEK").unwrap_or_default().parse::<f64>() {
                app.dev_seek(ms);
            }
            app.dev_select(std::env::var("SCREENCRAFT_SELECT").ok().as_deref());
        }
        app
    }

    /// Save a screenshot of the window to a PNG once things have settled (developer hook).
    fn dev_screenshot(&mut self, ctx: &egui::Context) {
        let Some(shot) = self.shot.as_mut() else { return };
        shot.frame += 1;
        ctx.request_repaint();
        if !shot.requested && shot.frame >= shot.after_frames {
            shot.requested = true;
            ctx.send_viewport_cmd(ViewportCommand::Screenshot(egui::UserData::default()));
        }
        let image = ctx.input(|i| {
            i.events.iter().find_map(|e| match e {
                egui::Event::Screenshot { image, .. } => Some(image.clone()),
                _ => None,
            })
        });
        if let Some(img) = image {
            let (w, h) = (img.width() as u32, img.height() as u32);
            let mut raw = Vec::with_capacity((w * h * 4) as usize);
            for c in &img.pixels {
                raw.extend_from_slice(&c.to_array());
            }
            if let Ok(f) = std::fs::File::create(&shot.path) {
                let mut enc = png::Encoder::new(std::io::BufWriter::new(f), w, h);
                enc.set_color(png::ColorType::Rgba);
                enc.set_depth(png::BitDepth::Eight);
                if let Ok(mut wr) = enc.write_header() {
                    let _ = wr.write_image_data(&raw);
                }
            }
            ctx.send_viewport_cmd(ViewportCommand::Close);
        }
    }

    // ---- recording flow ---------------------------------------------------------------------

    /// Press Record: checks, then the 3-2-1 countdown, then capture starts.
    fn begin_recording(&mut self, ctx: &egui::Context) {
        if self.rec.phase != Phase::Idle {
            return;
        }
        if self.rec.mode == Mode::Camera {
            if !self.prefs.camera {
                self.rec.show_error("Turn the camera on to record a talking-head video.");
                return;
            }
            if self.rec.live.loading(crate::live::Input::Camera, &self.prefs) {
                return;
            }
            if let Some(err) = self.rec.live.camera_error() {
                self.rec.show_error(err.to_string());
                return;
            }
        }
        if self.rec.mode == Mode::Window && self.rec.window.is_none() {
            self.rec.show_error("Choose a window to record (click Window).");
            return;
        }
        if self.rec.mode == Mode::Area && self.rec.area.is_none() {
            self.pick_area(ctx);
            return;
        }
        self.rec.close_menu();
        self.rec.phase = Phase::Countdown;
        self.rec.countdown_at = Some(Instant::now());
        ctx.request_repaint();
    }

    fn start_capture(&mut self, ctx: &egui::Context) {
        match self.rec.build_options(&self.prefs) {
            Ok(opts) => {
                self.prefs.save();
                self.rec.phase = Phase::Starting;
                self.rec.controller.start(opts);
            }
            Err(e) => {
                self.rec.phase = Phase::Idle;
                self.rec.show_error(e);
            }
        }
        ctx.request_repaint();
    }

    /// The note above the bar while recording more for a project, and the progress window
    /// while footage is being added.
    fn draw_append(&mut self, ctx: &egui::Context) {
        use std::sync::atomic::Ordering;
        if let Some((target, name)) = self.append_target.clone() {
            let active = matches!(self.rec.phase, Phase::Idle | Phase::Starting | Phase::Countdown | Phase::Recording | Phase::Confirm);
            if let (true, true, Some(o)) = (self.editors.is_empty(), active, ctx.input(|i| i.viewport().outer_rect)) {
                let size = Vec2::new(o.width().max(380.0), 34.0);
                let builder = ViewportBuilder::default()
                    .with_title("sc-append-note")
                    .with_position(Pos2::new(o.min.x, o.max.y - BAR_H - size.y - 8.0))
                    .with_inner_size(size)
                    .with_decorations(false)
                    .with_resizable(false)
                    .with_taskbar(false)
                    .with_always_on_top();
                let mut cancel = false;
                ctx.show_viewport_immediate(ViewportId::from_hash_of("sc-append-note"), builder, |ui, _| {
                    let r = ui.max_rect().expand(8.0);
                    ui.painter().rect_filled(r, 0.0, Color32::from_rgb(0x12, 0x2a, 0x45));
                    let inner = ui.max_rect();
                    ui.painter().text(
                        Pos2::new(inner.min.x + 6.0, inner.center().y),
                        egui::Align2::LEFT_CENTER,
                        format!("Recording more for \u{201c}{name}\u{201d}: it goes at the end"),
                        crate::kit::font(13.0),
                        Color32::WHITE,
                    );
                    let b = egui::Rect::from_min_size(Pos2::new(inner.max.x - 72.0, inner.center().y - 12.0), Vec2::new(70.0, 24.0));
                    if ui.put(b, egui::Button::new("Cancel")).clicked() {
                        cancel = true;
                    }
                });
                screencraft_capture::exclude_window_titled("sc-append-note");
                if cancel {
                    // back to the project; a recording already running is kept as its own project
                    self.append_target = None;
                    if self.rec.phase == Phase::Idle {
                        self.pending_open.push(target);
                    }
                }
            }
        }
        let Some(job) = &self.append_job else { return };
        let done = job.result.lock().unwrap().take();
        if let Some(res) = done {
            let job = self.append_job.take().unwrap();
            match res {
                Ok(a) => {
                    let _ = std::fs::remove_file(&a.old_screen);
                    if let Some(t) = &job.temp {
                        let _ = std::fs::remove_dir_all(t);
                    }
                    crate::applog::line(&format!("added {:.1} s to {}", a.added_ms / 1000.0, job.target.display()));
                }
                Err(e) => {
                    crate::applog::line(&format!("adding footage failed: {e}"));
                    if e != "cancelled" {
                        let kept = job.temp.as_ref().map(|t| format!(" (the new recording is kept as {})", t.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default())).unwrap_or_default();
                        self.rec.show_error(format!("Couldn't add the footage: {e}{kept}"));
                    }
                }
            }
            self.pending_open.push(job.target);
            return;
        }
        let f = job.progress.load(Ordering::Relaxed) as f32 / 1000.0;
        let cancel = job.cancel.clone();
        let builder = ViewportBuilder::default().with_title("Adding footage").with_inner_size([440.0, 150.0]).with_resizable(false);
        ctx.show_viewport_immediate(ViewportId::from_hash_of("sc-append-job"), builder, |ui, _| {
            egui::CentralPanel::default().frame(egui::Frame::new().fill(crate::theme::BG).inner_margin(egui::Margin::same(20))).show(ui, |ui| {
                ui.label(egui::RichText::new("Adding footage to the end of the project\u{2026}").size(14.0).color(Color32::WHITE));
                ui.add_space(10.0);
                ui.add(egui::ProgressBar::new(f).show_percentage());
                ui.add_space(10.0);
                if ui.button("Cancel").clicked() || ui.ctx().input(|i| i.viewport().close_requested()) {
                    cancel.store(true, Ordering::Relaxed);
                }
            });
        });
        ctx.request_repaint_after(Duration::from_millis(100));
    }

    fn finish_recording(&mut self, ctx: &egui::Context, discard: bool) {
        match self.rec.phase {
            Phase::Countdown => {
                self.rec.phase = Phase::Idle;
            }
            Phase::Recording | Phase::Confirm => {
                self.discard_after_stop = discard;
                self.rec.phase = Phase::Finishing;
                self.rec.controller.stop();
            }
            _ => {}
        }
        ctx.request_repaint();
    }

    fn pick_area(&mut self, ctx: &egui::Context) {
        let displays = self.rec.displays().to_vec();
        let d = displays.iter().find(|d| Some(d.id) == self.rec.display).or(displays.first()).cloned();
        self.area_picker = Some(crate::area::Picker::new(d, self.prefs.vertical.then_some(9.0 / 16.0)));
        ctx.request_repaint();
    }

    fn pump_events(&mut self, ctx: &egui::Context) {
        while let Ok(evt) = self.rec.controller.rx.try_recv() {
            match evt {
                Evt::Started { stats, width, height, encoder } => {
                    self.rec.stats = Some(stats);
                    self.rec.started_at = Some(Instant::now());
                    self.rec.paused = false;
                    self.rec.paused_at = None;
                    self.rec.paused_total = Duration::ZERO;
                    self.rec.size = (width, height);
                    self.rec.encoder = encoder;
                    self.rec.phase = Phase::Recording;
                }
                Evt::StartFailed(e) => {
                    self.rec.phase = Phase::Idle;
                    self.rec.show_error(e);
                }
                Evt::Stopped(r) => {
                    self.rec.phase = Phase::Idle;
                    self.rec.stats = None;
                    self.rec.started_at = None;
                    self.rec.paused = false;
                    self.rec.paused_at = None;
                    self.rec.paused_total = Duration::ZERO;
                    let discard = std::mem::take(&mut self.discard_after_stop);
                    if self.autorecord.is_some() {
                        match &r {
                            Ok(f) => eprintln!("[autorecord] finished: {} frames, {:.2}s, encoder {}, package {}", f.frames, f.duration_ms / 1000.0, f.encoder, f.package.dir.display()),
                            Err(e) => eprintln!("[autorecord] failed: {e}"),
                        }
                        ctx.send_viewport_cmd(ViewportCommand::Close);
                    }
                    match r {
                        Ok(f) if discard => {
                            let _ = std::fs::remove_dir_all(&f.package.dir);
                        }
                        Ok(f) => match self.append_target.take() {
                            Some((target, _)) => self.append_job = Some(AppendJob::start(target, f.package.dir.clone(), true)),
                            None => {
                                self.fresh.push(f.package.dir.clone());
                                self.pending_open.push(f.package.dir.clone());
                            }
                        },
                        Err(e) => self.rec.show_error(e),
                    }
                }
            }
        }
        // Global shortcut.
        if let Some(id) = self.hotkey_id {
            while let Ok(ev) = GlobalHotKeyEvent::receiver().try_recv() {
                if ev.id == id && ev.state == HotKeyState::Pressed {
                    match self.rec.phase {
                        Phase::Idle => self.begin_recording(ctx),
                        Phase::Recording => self.finish_recording(ctx, false),
                        Phase::Countdown => self.finish_recording(ctx, false),
                        _ => {}
                    }
                }
            }
        }
        // countdown done
        if self.rec.phase == Phase::Countdown {
            if self.rec.countdown_at.map(|t| t.elapsed() >= Duration::from_secs(3)).unwrap_or(false) {
                self.start_capture(ctx);
            }
        }
    }

    fn handle_action(&mut self, ctx: &egui::Context, a: Action) {
        match a {
            Action::None => {}
            Action::Record => self.begin_recording(ctx),
            Action::TogglePause => {
                if self.rec.phase == Phase::Recording {
                    self.rec.paused = !self.rec.paused;
                    if self.rec.paused {
                        self.rec.paused_at = Some(Instant::now());
                        self.rec.controller.set_paused(true);
                    } else {
                        if let Some(t) = self.rec.paused_at.take() {
                            self.rec.paused_total += t.elapsed();
                        }
                        self.rec.controller.set_paused(false);
                    }
                    ctx.request_repaint();
                }
            }
            Action::Stop => self.finish_recording(ctx, false),
            Action::Cancel => self.finish_recording(ctx, false),
            Action::AskDiscard => self.rec.phase = Phase::Confirm,
            Action::KeepRecording => self.rec.phase = Phase::Recording,
            Action::Discard => self.finish_recording(ctx, true),
            Action::Quit => {
                if !matches!(self.rec.phase, Phase::Recording | Phase::Countdown | Phase::Confirm) {
                    self.quit = true;
                    ctx.send_viewport_cmd(ViewportCommand::Close);
                }
            }
            Action::OpenProject(dir) => self.pending_open.push(dir),
            Action::PickArea => self.pick_area(ctx),
            Action::HoverWindow(w) => self.hover_window = w,
            Action::ShowCameraPreview => {
                self.cam_preview_hidden = false;
                self.cam_preview_anchor = None;
            }
        }
    }

    /// Where the idle bar of size `want` goes: back where the user last left it when that spot is
    /// still on a connected display (monitors come and go), otherwise centred on the main display.
    fn home_pos(&self, ctx: &egui::Context, want: Vec2) -> Pos2 {
        let displays = self.rec.displays();
        let rect = |d: &screencraft_capture::DisplayInfo| egui::Rect::from_min_size(Pos2::new(d.x as f32, d.y as f32), Vec2::new(d.width as f32, d.height as f32));
        if let Some([ax, ay]) = self.prefs.bar_anchor {
            let a = Pos2::new(ax, ay - 1.0);
            if let Some(r) = displays.iter().map(rect).find(|r| r.contains(a)) {
                let x = (ax - want.x / 2.0).clamp(r.min.x, (r.max.x - want.x).max(r.min.x));
                let y = (ay - want.y).clamp(r.min.y, (r.max.y - want.y).max(r.min.y));
                return Pos2::new(x, y);
            }
        }
        // The display list arrives a moment after start; until then use the monitor the window
        // opened on (the primary one, whose top-left is the desktop origin).
        let monitor = ctx.input(|i| i.viewport().monitor_size).filter(|m| m.x > 0.0 && m.y > 0.0);
        let r = displays
            .iter()
            .find(|d| d.is_main)
            .map(rect)
            .unwrap_or_else(|| egui::Rect::from_min_size(Pos2::ZERO, monitor.unwrap_or(Vec2::new(1440.0, 900.0))));
        Pos2::new(r.center().x - want.x / 2.0, r.center().y - want.y / 2.0)
    }

    /// The display a new window belongs on: the one holding the recorder bar's saved spot, else
    /// the main display (or, before the display list loads, the monitor the app opened on).
    fn home_display(&self, ctx: &egui::Context) -> egui::Rect {
        let rects: Vec<egui::Rect> = self
            .rec
            .displays()
            .iter()
            .map(|d| egui::Rect::from_min_size(Pos2::new(d.x as f32, d.y as f32), Vec2::new(d.width as f32, d.height as f32)))
            .collect();
        if let Some([ax, ay]) = self.prefs.bar_anchor {
            if let Some(r) = rects.iter().find(|r| r.contains(Pos2::new(ax, ay - 1.0))) {
                return *r;
            }
        }
        let main = self.rec.displays().iter().position(|d| d.is_main);
        if let Some(r) = main.and_then(|i| rects.get(i)) {
            return *r;
        }
        let monitor = ctx.input(|i| i.viewport().monitor_size).filter(|m| m.x > 0.0 && m.y > 0.0);
        egui::Rect::from_min_size(Pos2::ZERO, monitor.unwrap_or(Vec2::new(1440.0, 900.0)))
    }

    /// Size, position and minimum size for a window that would like to be `want` but must fit
    /// on its display (a 1280x800 laptop can't hold a 1440x900 editor), centred there.
    pub(crate) fn fit_window(&self, ctx: &egui::Context, want: Vec2, min: Vec2) -> (Vec2, Pos2, Vec2) {
        let d = self.home_display(ctx);
        // leave room for the taskbar and the window frame
        let room = Vec2::new((d.width() - 24.0).max(320.0), (d.height() - 96.0).max(240.0));
        let size = want.min(room);
        let min = min.min(size);
        let pos = Pos2::new(d.center().x - size.x / 2.0, (d.min.y + (d.height() - 48.0 - size.y) / 2.0).max(d.min.y));
        (size, pos, min)
    }

    /// Remember where the user dragged the idle bar, so the next start puts it back there.
    fn remember_bar(&mut self, ctx: &egui::Context) {
        if self.rec.phase != Phase::Idle || !self.editors.is_empty() || self.rec.menu.is_some() || self.rec.displays().is_empty() {
            return;
        }
        let Some((want, _, _)) = self.applied else { return };
        let (outer, dragging) = ctx.input(|i| (i.viewport().outer_rect, i.pointer.any_down()));
        let Some(o) = outer else { return };
        // only a settled, full-size bar on a real display (not mid-resize, not parked off screen)
        if dragging || (o.width() - want.x).abs() > 2.0 || (o.height() - want.y).abs() > 2.0 {
            return;
        }
        let anchor = Pos2::new(o.center().x, o.max.y);
        let on_screen = self.rec.displays().iter().any(|d| {
            egui::Rect::from_min_size(Pos2::new(d.x as f32, d.y as f32), Vec2::new(d.width as f32, d.height as f32)).contains(Pos2::new(anchor.x, anchor.y - 1.0))
        });
        let moved = self.prefs.bar_anchor.map(|[x, y]| (x - anchor.x).abs() > 2.0 || (y - anchor.y).abs() > 2.0).unwrap_or(true);
        if on_screen && moved {
            self.prefs.bar_anchor = Some([anchor.x, anchor.y]);
            self.prefs.save();
        }
    }

    /// Keep the bar's window the size the current phase wants, anchored at its bottom centre.
    fn place_bar(&mut self, ctx: &egui::Context) {
        let screen_w = self
            .rec
            .displays()
            .iter()
            .find(|d| d.is_main)
            .map(|d| d.width as f32)
            .or_else(|| ctx.input(|i| i.viewport().monitor_size).map(|m| m.x).filter(|w| *w > 0.0))
            .unwrap_or(1440.0);
        let camera = self.prefs.camera;
        // While an editor is open the bar steps aside (a 1px window parked off screen keeps the
        // app's event loop running); it comes back when the last editor closes.
        if !self.editors.is_empty() && self.rec.phase == Phase::Idle {
            let key = (Vec2::splat(1.0), Phase::Idle, true);
            if self.applied != Some(key) {
                ctx.send_viewport_cmd(ViewportCommand::InnerSize(Vec2::splat(1.0)));
                ctx.send_viewport_cmd(ViewportCommand::OuterPosition(Pos2::new(-20000.0, -20000.0)));
                self.applied = Some(key);
            }
            return;
        }
        if matches!(self.applied, Some((v, _, true)) if v == Vec2::splat(1.0)) {
            self.applied = None;
        }
        let want = self.rec.window_size(screen_w, camera);
        let key = (want, self.rec.phase, self.rec.menu.is_some());
        if self.applied == Some(key) {
            return;
        }
        let (outer, first) = ctx.input(|i| (i.viewport().outer_rect, self.applied.is_none()));
        let docked = matches!(self.rec.phase, Phase::Idle | Phase::Starting);
        let pos = match (outer, first, docked) {
            (Some(o), false, _) => {
                // keep the bottom centre where it is
                let cx = o.center().x;
                let bottom = o.max.y;
                Pos2::new(cx - want.x / 2.0, bottom - want.y)
            }
            _ => self.home_pos(ctx, want),
        };
        // after a recording the bar returns to its dock
        let pos = if docked && matches!(self.applied, Some((_, p, _)) if !matches!(p, Phase::Idle | Phase::Starting)) {
            self.home_pos(ctx, want)
        } else {
            pos
        };
        ctx.send_viewport_cmd(ViewportCommand::InnerSize(want));
        ctx.send_viewport_cmd(ViewportCommand::OuterPosition(pos));
        self.applied = Some(key);
    }

    /// Floating opaque live viewfinder shown in Camera-only mode (kept out of capture).
    fn draw_camera_preview(&mut self, ctx: &egui::Context) {
        let active = self.editors.is_empty()
            && self.rec.mode == Mode::Camera
            && self.prefs.camera
            && !self.cam_preview_hidden
            && matches!(self.rec.phase, Phase::Idle | Phase::Countdown | Phase::Starting | Phase::Recording | Phase::Confirm);
        if !active {
            self.cam_preview_anchor = None;
            return;
        }
        let tex = self.rec.live.preview_texture(self.prefs.vertical);
        let aspect = match tex {
            Some((_, cw, ch)) => (cw as f32 / ch.max(1) as f32).clamp(0.4, 2.5),
            None => if self.prefs.vertical { 9.0 / 16.0 } else { 16.0 / 9.0 },
        };
        let mut size = if self.prefs.vertical || aspect < 1.0 {
            Vec2::new((360.0 * aspect).clamp(180.0, 420.0), 360.0)
        } else {
            Vec2::new(384.0, (384.0 / aspect).clamp(180.0, 320.0))
        };
        let disp = self.home_display(ctx);
        let max_room = Vec2::new((disp.width() - 48.0).max(200.0), (disp.height() - BAR_H - 96.0).max(160.0));
        if size.x > max_room.x || size.y > max_room.y {
            let k = (max_room.x / size.x).min(max_room.y / size.y);
            size *= k;
        }
        let default_pos = self.cam_preview_anchor.unwrap_or_else(|| {
            let pos = match ctx.input(|i| i.viewport().outer_rect) {
                Some(bar_o) => Pos2::new(
                    (bar_o.center().x - size.x / 2.0).clamp(disp.min.x + 8.0, (disp.max.x - size.x - 8.0).max(disp.min.x + 8.0)),
                    (bar_o.min.y - size.y - 14.0).max(disp.min.y + 12.0),
                ),
                None => Pos2::new(disp.center().x - size.x / 2.0, (disp.center().y - size.y / 2.0 - 60.0).max(disp.min.y + 12.0)),
            };
            self.cam_preview_anchor = Some(pos);
            pos
        });
        let builder = ViewportBuilder::default()
            .with_title("sc-camera-preview")
            .with_position(default_pos)
            .with_inner_size(size)
            .with_decorations(false)
            .with_resizable(false)
            .with_has_shadow(true)
            .with_taskbar(false)
            .with_always_on_top();
        let vertical = self.prefs.vertical;
        let phase = self.rec.phase;
        let paused = self.rec.paused;
        let elapsed_ms = self.rec.elapsed_ms();
        let countdown_left = self.rec.countdown_at.map(|t| (3.0 - t.elapsed().as_secs_f32()).ceil().max(1.0) as u32);
        let cam_loading = self.rec.live.loading(crate::live::Input::Camera, &self.prefs);
        let cam_err = self.rec.live.camera_error().map(str::to_string);
        let mut hide_req = false;
        let mut retry_req = false;
        ctx.show_viewport_immediate(ViewportId::from_hash_of("sc-camera-preview"), builder, |ui, _| {
            egui::CentralPanel::default().frame(egui::Frame::NONE).show(ui, |ui| {
                let full = ui.max_rect();
                ui.painter().rect_filled(full, egui::CornerRadius::same(10), crate::theme::BG);

                let close_r = egui::Rect::from_min_size(Pos2::new(full.max.x - 32.0, full.min.y + 8.0), Vec2::splat(24.0));
                let drag_resp = ui.interact(full, ui.id().with("cam-prev-drag"), egui::Sense::drag());
                let close_resp = ui.interact(close_r, ui.id().with("cam-prev-close"), egui::Sense::click());
                if drag_resp.drag_started() && !close_resp.hovered() {
                    ui.ctx().send_viewport_cmd(ViewportCommand::StartDrag);
                }

                if let Some((tid, cw, ch)) = tex {
                    let uv = egui::Rect::from_min_max(Pos2::new(1.0, 0.0), Pos2::new(0.0, 1.0));
                    ui.painter().image(tid, full, uv, Color32::WHITE);
                    let secs = (elapsed_ms / 1000.0) as u64;
                    let rec_on = matches!(phase, Phase::Recording | Phase::Confirm);
                    let badge = match phase {
                        Phase::Recording | Phase::Confirm if paused => format!("PAUSED  {}:{:02}", secs / 60, secs % 60),
                        Phase::Recording | Phase::Confirm => format!("REC  {}:{:02}", secs / 60, secs % 60),
                        _ if vertical => format!("Webcam · 9:16 · {cw}×{ch}"),
                        _ => format!("Webcam · {cw}×{ch}"),
                    };
                    let dot_w = if rec_on { 14.0 } else { 0.0 };
                    let tw = crate::kit::text_width(ui, &badge, 11.5);
                    let br = egui::Rect::from_min_size(Pos2::new(full.min.x + 8.0, full.min.y + 8.0), Vec2::new(tw + 16.0 + dot_w, 22.0));
                    let p = ui.painter();
                    p.rect_filled(br, egui::CornerRadius::same(6), Color32::from_rgba_unmultiplied(11, 16, 26, 200));
                    if rec_on {
                        let dot_col = if paused { Color32::from_rgb(0xf6, 0xc4, 0x53) } else { crate::theme::DANGER };
                        p.circle_filled(Pos2::new(br.min.x + 11.0, br.center().y), 4.0, dot_col);
                    }
                    p.text(Pos2::new(br.min.x + 8.0 + dot_w, br.center().y), egui::Align2::LEFT_CENTER, badge, crate::kit::font(11.5), Color32::WHITE);
                } else if cam_loading {
                    let t = ui.input(|i| i.time) as f32;
                    let c = full.center() - Vec2::new(0.0, 14.0);
                    {
                        let p = ui.painter();
                        for k in 0..12 {
                            let a = k as f32 / 12.0 * std::f32::consts::TAU + t * 8.0;
                            let alpha_ = ((k as f32 / 12.0) * 200.0) as u8;
                            p.circle_filled(c + Vec2::new(a.cos(), a.sin()) * 11.0, 1.8, Color32::from_rgba_unmultiplied(106, 174, 232, alpha_));
                        }
                        p.text(full.center() + Vec2::new(0.0, 14.0), egui::Align2::CENTER_CENTER, "Starting camera…", crate::kit::font(13.0), crate::theme::LABEL);
                    }
                    ui.ctx().request_repaint_after(Duration::from_millis(40));
                } else if let Some(err) = &cam_err {
                    let inner = full.shrink2(Vec2::new(22.0, 38.0));
                    ui.scope_builder(egui::UiBuilder::new().max_rect(inner), |ui| {
                        ui.vertical_centered(|ui| {
                            ui.add_space((inner.height() - 68.0).max(0.0) / 2.0);
                            ui.add(egui::Label::new(egui::RichText::new(err).size(12.5).color(crate::theme::TEXT)).wrap());
                            ui.add_space(10.0);
                            if ui.button("Retry camera").clicked() {
                                retry_req = true;
                            }
                        });
                    });
                } else {
                    ui.painter().text(full.center(), egui::Align2::CENTER_CENTER, "Waiting for camera…", crate::kit::font(13.0), crate::theme::MUTED);
                }

                let chov = close_resp.hovered();
                {
                    let p = ui.painter();
                    if phase == Phase::Countdown {
                        if let Some(n) = countdown_left {
                            p.circle_filled(full.center(), 32.0, Color32::from_rgba_unmultiplied(11, 16, 26, 200));
                            p.text(full.center(), egui::Align2::CENTER_CENTER, n.to_string(), egui::FontId::proportional(34.0), Color32::WHITE);
                        }
                    }
                    p.rect_filled(close_r, egui::CornerRadius::same(6), if chov { crate::theme::alpha(crate::theme::DANGER, 0.85) } else { Color32::from_rgba_unmultiplied(11, 16, 26, 195) });
                    let cc = close_r.center();
                    for (a, b) in [((-4.0, -4.0), (4.0, 4.0)), ((4.0, -4.0), (-4.0, 4.0))] {
                        p.line_segment([cc + Vec2::new(a.0, a.1), cc + Vec2::new(b.0, b.1)], egui::Stroke::new(1.8, Color32::WHITE));
                    }
                    p.rect_stroke(full, egui::CornerRadius::same(10), egui::Stroke::new(1.0, crate::theme::LINE_2), egui::StrokeKind::Inside);
                }
                if close_resp.on_hover_text("Hide camera preview (click Camera in the bar to reopen)").on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                    hide_req = true;
                } else if drag_resp.hovered() {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
                }
            });
        });
        screencraft_capture::exclude_window_titled("sc-camera-preview");
        if hide_req {
            self.cam_preview_hidden = true;
            self.cam_preview_anchor = None;
        }
        if retry_req {
            self.rec.live.retry_camera();
            self.rec.probe_cameras();
        }
    }
}

impl eframe::App for App {
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        [0.0, 0.0, 0.0, 0.0]
    }

    fn logic(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        #[cfg(windows)]
        if self.native_window.is_none() {
            use raw_window_handle::{HasWindowHandle, RawWindowHandle};
            if let Ok(h) = frame.window_handle() {
                if let RawWindowHandle::Win32(w) = h.as_raw() {
                    self.native_window = Some(w.hwnd.get());
                    screencraft_capture::set_excluded_from_capture(w.hwnd.get(), true);
                }
            }
        }
        #[cfg(not(windows))]
        let _ = frame;
        self.pump_events(ctx);
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.draw(ui);
    }

    fn on_exit(&mut self) {
        for e in &mut self.editors {
            e.ed.pause();
            e.ed.save_now();
        }
        self.prefs.save();
    }
}

impl App {
    /// One frame of the whole application UI.
    pub fn draw(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        self.dev_screenshot(&ctx);
        self.sync_hotkey();
        self.rec.parked = !self.editors.is_empty();
        self.rec.refresh(&mut self.prefs);
        if let Some(secs) = self.autorecord {
            match self.rec.phase {
                Phase::Idle if !self.autorecord_started => {
                    self.autorecord_started = true;
                    self.start_capture(&ctx);
                }
                Phase::Recording if self.rec.elapsed_ms() >= secs * 1000.0 => self.finish_recording(&ctx, false),
                _ => {}
            }
            if self.rec.error.is_some() && self.rec.phase == Phase::Idle && self.autorecord_started {
                eprintln!("[autorecord] error: {}", self.rec.error.as_ref().map(|e| e.0.clone()).unwrap_or_default());
                ctx.send_viewport_cmd(ViewportCommand::Close);
            }
            ctx.request_repaint_after(Duration::from_millis(100));
        }
        if self.rec.phase == Phase::Idle && ctx.input(|i| i.key_pressed(egui::Key::Escape)) && self.rec.menu.is_some() {
            self.rec.close_menu();
        }
        if matches!(self.rec.phase, Phase::Countdown) && ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.finish_recording(&ctx, false);
        }

        if self.headless {
            // screenshots and tests: the first editor fills the window, else the bar is drawn on a backdrop
            if let Some(e) = self.editors.first_mut() {
                e.ed.ui(ui, &mut self.prefs);
                return;
            }
            let rect = ui.max_rect();
            ui.painter().rect_filled(rect, 0.0, crate::theme::BG);
            let w = self.rec.bar_width().min(rect.width() - 24.0);
            let menu_h = if self.rec.menu.is_some() { 360.0 } else { 0.0 };
            let bar_area = egui::Rect::from_min_size(Pos2::new(rect.center().x - w / 2.0, rect.max.y - 90.0 - BAR_H - menu_h), Vec2::new(w, BAR_H + menu_h));
            let mut child = ui.new_child(egui::UiBuilder::new().max_rect(bar_area));
            let a = self.rec.show(&mut child, &mut self.prefs);
            self.handle_action(&ctx, a);
            return;
        }

        self.place_bar(&ctx);
        self.remember_bar(&ctx);
        let a = egui::CentralPanel::default().frame(egui::Frame::NONE).show(ui, |ui| self.rec.show(ui, &mut self.prefs)).inner;
        self.handle_action(&ctx, a);

        if ctx.input(|i| (i.modifiers.alt && i.key_pressed(egui::Key::F4)) || i.viewport().close_requested()) {
            if self.editors.iter().any(|e| e.ed.dirty) {
                ctx.send_viewport_cmd(ViewportCommand::CancelClose);
                self.quit_pending = true;
            } else {
                self.quit = true;
            }
        } else if self.editors.is_empty() && ctx.input(|i| i.modifiers.ctrl && !i.modifiers.alt && i.key_pressed(egui::Key::F4)) {
            self.quit = true;
        }

        // editors, one window each
        let mut open_requests: Vec<std::path::PathBuf> = Vec::new();
        let mut compress_requests: Vec<std::path::PathBuf> = Vec::new();
        let mut quit_requested = false;
        let mut i = 0;
        while i < self.editors.len() {
            let (id, title) = (self.editors[i].id, self.editors[i].title.clone());
            let (size, pos, min) = self.fit_window(&ctx, Vec2::new(1440.0, 900.0), Vec2::new(980.0, 620.0));
            let builder = ViewportBuilder::default().with_title(title).with_inner_size(size).with_position(pos).with_min_inner_size(min);
            let mut close = false;
            {
                let ed = &mut self.editors[i].ed;
                ctx.show_viewport_immediate(id, builder, |ui, _class| {
                    ed.ui(ui, &mut self.prefs);
                    let (alt_f4, ctrl_f4, close_req) = ui.ctx().input(|i| {
                        let alt = i.modifiers.alt && i.key_pressed(egui::Key::F4);
                        let ctrl = i.modifiers.ctrl && !i.modifiers.alt && i.key_pressed(egui::Key::F4);
                        let req = i.viewport().close_requested();
                        (alt || (req && i.modifiers.alt), ctrl, req)
                    });
                    if alt_f4 {
                        ui.ctx().send_viewport_cmd(ViewportCommand::CancelClose);
                        quit_requested = true;
                    } else if ctrl_f4 || close_req {
                        if ed.dirty {
                            ui.ctx().send_viewport_cmd(ViewportCommand::CancelClose);
                            ed.close_prompt = Some(crate::editor::CloseIntent::Window);
                        } else {
                            close = true;
                        }
                    }
                    if let Some(req) = ed.open_request.take() {
                        open_requests.push(req);
                    }
                    if let Some(req) = ed.compress_request.take() {
                        compress_requests.push(req);
                    }
                });
            }
            if quit_requested {
                self.quit_pending = true;
                quit_requested = false;
            }
            if let Some(req) = self.editors[i].ed.footage_request.take() {
                use crate::editor::FootageRequest;
                let dir = self.editors[i].ed.package.dir.clone();
                let name = self.editors[i].ed.project.name.clone();
                match req {
                    FootageRequest::NewRecording => self.append_target = None,
                    FootageRequest::RecordMore => self.append_target = Some((dir, name)),
                    FootageRequest::Add(p) => self.append_job = Some(AppendJob::start(dir, p, false)),
                }
                close = true;
            }
            let title = format!("{}{}", self.editors[i].ed.project.name, if self.editors[i].ed.dirty { " *" } else { "" });
            if title != self.editors[i].title {
                self.editors[i].title = title;
            }
            // the answer to a save prompt
            if let Some(ans) = self.editors[i].ed.close_answer.take() {
                use crate::editor::{Answer, CloseIntent};
                let intent = self.editors[i].ed.close_prompt.take();
                match ans {
                    Answer::Cancel => {
                        if matches!(intent, Some(CloseIntent::Quit)) {
                            self.quit_pending = false;
                        }
                    }
                    Answer::Save | Answer::Discard => {
                        if ans == Answer::Save {
                            self.editors[i].ed.save_now();
                        } else {
                            self.editors[i].ed.dirty = false;
                        }
                        match intent {
                            Some(CloseIntent::Window | CloseIntent::Quit) => close = true,
                            Some(CloseIntent::Open(p)) => {
                                self.editors[i].ed.close_after_open = true;
                                open_requests.push(p);
                            }
                            None => {}
                        }
                    }
                }
            }
            if self.editors[i].ed.discard_request {
                let mut w = self.editors.remove(i);
                let dir = w.ed.package.dir.clone();
                w.ed.pause();
                w.ed.export.cancel();
                drop(w);
                self.fresh.retain(|d| *d != dir);
                if self.append_target.as_ref().is_some_and(|(d, _)| *d == dir) {
                    self.append_target = None;
                }
                if let Err(e) = remove_package_dir(&dir) {
                    crate::applog::line(&format!("could not discard {}: {e}", dir.display()));
                    self.rec.show_error(format!("Couldn't delete the recording: {e}"));
                } else {
                    crate::applog::line(&format!("discarded {}", dir.display()));
                }
                ctx.request_repaint();
            } else if close {
                let mut w = self.editors.remove(i);
                w.ed.pause();
                w.ed.save_now();
            } else {
                i += 1;
            }
        }
        if let Some(p) = compress_requests.pop() {
            self.compressor = Some(crate::compressor::Compressor::new(Some(p)));
        }
        if let Some(mut c) = self.compressor.take() {
            if c.show(&ctx) {
                self.compressor = Some(c);
            }
        }
        self.draw_append(&ctx);
        self.draw_camera_preview(&ctx);
        open_requests.extend(std::mem::take(&mut self.pending_open));
        for dir in open_requests {
            // opening another project replaces the ones on screen, asking about unsaved edits first
            if self.editors.iter().any(|e| e.ed.package.dir == dir) {
                continue;
            }
            if let Some(e) = self.editors.iter_mut().find(|e| e.ed.dirty && !e.ed.close_after_open) {
                e.ed.close_prompt = Some(crate::editor::CloseIntent::Open(dir));
                continue;
            }
            for mut w in self.editors.drain(..) {
                w.ed.pause();
            }
            self.open_package(&ctx, dir);
        }

        // area picker + frame outline
        if let Some(mut p) = self.area_picker.take() {
            match p.show(&ctx) {
                crate::area::Outcome::Pending => self.area_picker = Some(p),
                crate::area::Outcome::Picked(rect) => {
                    self.rec.area = Some(rect);
                    self.rec.mode = Mode::Area;
                    self.rec.display = p.display_id().or(self.rec.display);
                }
                crate::area::Outcome::Cancelled => {
                    if self.rec.area.is_none() {
                        self.rec.mode = Mode::Display;
                    }
                }
            }
        }
        let outline: Option<(PRect, crate::area::FrameStyle)> = match (self.rec.phase, self.rec.mode, self.rec.area, &self.hover_window) {
            (Phase::Recording | Phase::Countdown | Phase::Starting | Phase::Confirm, Mode::Area, Some(a), _) => Some((a, crate::area::FrameStyle::Rec)),
            // a window or a whole display: outline it too, so it is clear what is being recorded
            // (the app's own windows are excluded from the capture, so this never appears in it)
            (Phase::Recording | Phase::Countdown | Phase::Starting | Phase::Confirm, Mode::Window, _, _) => self
                .rec
                .window
                .and_then(|id| self.rec.windows().iter().find(|w| w.id == id))
                .map(|w| (PRect { x: w.x, y: w.y, width: w.width, height: w.height }, crate::area::FrameStyle::Rec)),
            (Phase::Recording | Phase::Countdown | Phase::Starting | Phase::Confirm, Mode::Display, _, _) => {
                let ds = self.rec.displays();
                self.rec
                    .display
                    .and_then(|id| ds.iter().find(|d| d.id == id))
                    .or_else(|| ds.iter().find(|d| d.is_main))
                    .or(ds.first())
                    .map(|d| (PRect { x: d.x, y: d.y, width: d.width, height: d.height }, crate::area::FrameStyle::Rec))
            }
            (Phase::Idle, _, _, Some(w)) => Some((PRect { x: w.x, y: w.y, width: w.width, height: w.height }, crate::area::FrameStyle::Highlight)),
            (Phase::Idle, Mode::Area, Some(a), _) if self.area_picker.is_none() => Some((a, crate::area::FrameStyle::Selection)),
            _ => None,
        };
        if let Some((rect, style)) = outline {
            crate::area::draw_frame(&ctx, rect, style, self.rec.displays());
        }
        if self.quit_pending {
            if self.editors.iter().any(|e| e.ed.dirty) {
                if !self.editors.iter().any(|e| e.ed.close_prompt.is_some()) {
                    if let Some(e) = self.editors.iter_mut().find(|e| e.ed.dirty) {
                        e.ed.close_prompt = Some(crate::editor::CloseIntent::Quit);
                    }
                }
            } else {
                self.quit_pending = false;
                self.quit = true;
            }
        }
        if self.quit {
            ctx.send_viewport_cmd(ViewportCommand::Close);
        }
    }
}

/// Delete a `.screencraft` package folder, retrying briefly on Windows in case a background
/// reader (e.g. waveform scan) is still releasing its file handle.
fn remove_package_dir(dir: &std::path::Path) -> std::io::Result<()> {
    let mut last_err = None;
    for attempt in 0..10 {
        match std::fs::remove_dir_all(dir) {
            Ok(()) => return Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => {
                last_err = Some(e);
                if attempt < 9 {
                    std::thread::sleep(Duration::from_millis(40));
                }
            }
        }
    }
    Err(last_err.unwrap())
}
