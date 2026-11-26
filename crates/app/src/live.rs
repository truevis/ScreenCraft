//! Live inputs of the recorder: the camera picture shown in the bar (and fed to the recorder)
//! and the microphone level meter, started and stopped to match the current selections.

use crate::prefs::Prefs;
use crate::recorder::Phase;
use eframe::egui;
use screencraft_audio::{AudioBlock, AudioCapture, CaptureConfig, Clock};
use screencraft_capture::camera::{CamFrame, CamPreset, CameraHub, CameraStream};
#[cfg(target_os = "macos")]
use screencraft_capture::{create_capturer, CaptureRequest, Capturer, Event, Target};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Input {
    Camera,
    Mic,
    Sys,
}

struct Started<T> {
    key: Option<String>,
    handle: T,
}

enum SysMeter {
    Audio(AudioCapture),
    #[cfg(target_os = "macos")]
    Capturer(Box<dyn Capturer>),
}

impl SysMeter {
    fn stop(self) {
        match self {
            SysMeter::Audio(a) => a.stop(),
            #[cfg(target_os = "macos")]
            SysMeter::Capturer(mut c) => c.stop(),
        }
    }
}

pub struct Live {
    ctx: egui::Context,
    hub: Arc<CameraHub>,
    camera: Option<Started<CameraStream>>,
    camera_preset: CamPreset,
    camera_starting: Arc<AtomicBool>,
    camera_slot: Arc<Mutex<Option<Result<Started<CameraStream>, String>>>>,
    camera_failed: Option<String>,
    camera_attempted: Option<(Option<String>, CamPreset)>,
    tex: Option<egui::TextureHandle>,
    last_frame: usize,
    preview_tex: Option<egui::TextureHandle>,
    last_preview: (usize, bool),
    mic: Option<Started<AudioCapture>>,
    mic_starting: Arc<AtomicBool>,
    mic_slot: Arc<Mutex<Option<Result<Started<AudioCapture>, String>>>>,
    mic_level: Arc<AtomicU32>,
    /// Level meter for the computer's own audio: WASAPI loopback on Windows, ScreenCaptureKit on macOS.
    sys: Option<SysMeter>,
    sys_starting: Arc<AtomicBool>,
    sys_slot: Arc<Mutex<Option<Result<SysMeter, String>>>>,
    sys_level: Arc<AtomicU32>,
    /// The meter could not start; do not retry until the input is switched off and on again.
    sys_failed: bool,
    error: Arc<Mutex<Option<String>>>,
}

fn set_peak(slot: &AtomicU32, v: f32) {
    let old = f32::from_bits(slot.load(Ordering::Relaxed));
    let new = if v > old { v } else { old * 0.92 + v * 0.08 };
    slot.store(new.to_bits(), Ordering::Relaxed);
}

impl Live {
    pub fn new(ctx: &egui::Context) -> Self {
        Self {
            ctx: ctx.clone(),
            hub: CameraHub::new(),
            camera: None,
            camera_preset: CamPreset::Bubble,
            camera_starting: Arc::new(AtomicBool::new(false)),
            camera_slot: Arc::new(Mutex::new(None)),
            camera_failed: None,
            camera_attempted: None,
            tex: None,
            last_frame: 0,
            preview_tex: None,
            last_preview: (0, false),
            mic: None,
            mic_starting: Arc::new(AtomicBool::new(false)),
            mic_slot: Arc::new(Mutex::new(None)),
            mic_level: Arc::new(AtomicU32::new(0)),
            sys: None,
            sys_starting: Arc::new(AtomicBool::new(false)),
            sys_slot: Arc::new(Mutex::new(None)),
            sys_level: Arc::new(AtomicU32::new(0)),
            sys_failed: false,
            error: Arc::new(Mutex::new(None)),
        }
    }

    pub fn camera_source(&self) -> Arc<CameraHub> {
        self.hub.clone()
    }

    pub fn take_error(&mut self) -> Option<String> {
        self.error.lock().unwrap().take()
    }

    pub fn loading(&self, which: Input, prefs: &Prefs) -> bool {
        match which {
            Input::Camera => prefs.camera && self.camera_starting.load(Ordering::SeqCst),
            Input::Mic => prefs.microphone && self.mic_starting.load(Ordering::SeqCst),
            Input::Sys => false,
        }
    }

    pub fn any_loading(&self, prefs: &Prefs) -> bool {
        self.loading(Input::Camera, prefs) || self.loading(Input::Mic, prefs)
    }

    pub fn levels(&self) -> (f32, f32) {
        (f32::from_bits(self.mic_level.load(Ordering::Relaxed)), f32::from_bits(self.sys_level.load(Ordering::Relaxed)))
    }

    pub fn retry_camera(&mut self) {
        self.camera_failed = None;
        self.camera_attempted = None;
    }

    pub fn camera_error(&self) -> Option<&str> {
        self.camera_failed.as_deref()
    }

    pub fn camera_frame_size(&self) -> Option<(u32, u32)> {
        self.hub.latest().map(|f| (f.width, f.height))
    }

    /// Start or stop the camera and microphone meter so they match `prefs` and the phase.
    pub fn sync(&mut self, prefs: &Prefs, phase: Phase, parked: bool, preset: CamPreset, want_system_meter: bool) {
        // finished background starts
        if let Some(r) = self.camera_slot.lock().unwrap().take() {
            match r {
                Ok(s) => {
                    self.camera_failed = None;
                    self.camera = Some(s);
                }
                Err(e) => {
                    self.camera_failed = Some(e.clone());
                    *self.error.lock().unwrap() = Some(e);
                }
            }
        }
        if let Some(r) = self.mic_slot.lock().unwrap().take() {
            match r {
                Ok(s) => self.mic = Some(s),
                Err(e) => *self.error.lock().unwrap() = Some(e),
            }
        }
        if let Some(r) = self.sys_slot.lock().unwrap().take() {
            // a meter that cannot start (no permission yet, say) just stays at zero
            match r {
                Ok(c) => self.sys = Some(c),
                Err(_) => self.sys_failed = true,
            }
        }
        if std::env::var_os("SCREENCRAFT_NO_LIVE").is_some() {
            return;
        }
        let in_use = matches!(phase, Phase::Idle | Phase::Countdown | Phase::Recording | Phase::Confirm | Phase::Starting);
        // camera
        let want_cam = prefs.camera && in_use;
        let cam_key = prefs.camera_id.clone();
        if !want_cam {
            self.camera_failed = None;
            self.camera_attempted = None;
        } else if self.camera_attempted.as_ref() != Some(&(cam_key.clone(), preset)) && phase == Phase::Idle {
            self.camera_failed = None;
        }
        if let Some(c) = &self.camera {
            if !want_cam || c.key != cam_key || (self.camera_preset != preset && phase == Phase::Idle) {
                if let Some(c) = self.camera.take() {
                    c.handle.stop();
                }
                self.hub.clear();
                self.tex = None;
                self.preview_tex = None;
                self.last_preview = (0, false);
            }
        }
        if want_cam && self.camera.is_none() && self.camera_failed.is_none() && !self.camera_starting.swap(true, Ordering::SeqCst) {
            self.camera_preset = preset;
            self.camera_attempted = Some((cam_key.clone(), preset));
            let (slot, flag, hub, ctx, id) = (self.camera_slot.clone(), self.camera_starting.clone(), self.hub.clone(), self.ctx.clone(), cam_key.clone());
            std::thread::spawn(move || {
                let h2 = hub.clone();
                let ctx2 = ctx.clone();
                let r = screencraft_capture::camera::start(id.as_deref(), preset, move |f: CamFrame| {
                    h2.publish(f);
                    ctx2.request_repaint();
                })
                .map(|handle| Started { key: id, handle });
                *slot.lock().unwrap() = Some(r);
                flag.store(false, Ordering::SeqCst);
                ctx.request_repaint();
            });
        }
        // microphone meter (the recording reads the microphone itself while it runs)
        let want_mic = prefs.microphone && phase == Phase::Idle;
        let mic_key = prefs.microphone_id.clone();
        if let Some(m) = &self.mic {
            if !want_mic || m.key != mic_key {
                if let Some(m) = self.mic.take() {
                    m.handle.stop();
                }
                self.mic_level.store(0, Ordering::Relaxed);
            }
        }
        if want_mic && self.mic.is_none() && !self.mic_starting.swap(true, Ordering::SeqCst) {
            let (slot, flag, level, ctx, id) = (self.mic_slot.clone(), self.mic_starting.clone(), self.mic_level.clone(), self.ctx.clone(), mic_key);
            std::thread::spawn(move || {
                let (tx, rx) = std::sync::mpsc::channel::<AudioBlock>();
                let clock = Clock::start();
                let cfg = CaptureConfig { microphone: id.clone(), use_microphone: true, system: None, use_system: false };
                let r = AudioCapture::start(&cfg, &clock, tx).map(|handle| Started { key: id, handle });
                if r.is_ok() {
                    let ctx2 = ctx.clone();
                    std::thread::spawn(move || {
                        while let Ok(b) = rx.recv() {
                            set_peak(&level, screencraft_audio::mix::peak(&b.data));
                            ctx2.request_repaint_after(std::time::Duration::from_millis(40));
                        }
                    });
                }
                *slot.lock().unwrap() = Some(r);
                flag.store(false, Ordering::SeqCst);
                ctx.request_repaint();
            });
        }
        // system-audio meter: only while the bar is showing and the input is active
        let want_sys = want_system_meter && prefs.system_audio && phase == Phase::Idle && !parked;
        if !want_sys {
            self.sys_failed = false;
            if let Some(c) = self.sys.take() {
                c.stop();
                self.sys_level.store(0, Ordering::Relaxed);
            }
        }
        if want_sys && self.sys.is_none() && !self.sys_failed && !self.sys_starting.swap(true, Ordering::SeqCst) {
            let (slot, flag, level, ctx) = (self.sys_slot.clone(), self.sys_starting.clone(), self.sys_level.clone(), self.ctx.clone());
            std::thread::spawn(move || {
                #[cfg(not(target_os = "macos"))]
                let r = {
                    let (tx, rx) = std::sync::mpsc::channel::<AudioBlock>();
                    let clock = Clock::start();
                    let cfg = CaptureConfig { microphone: None, use_microphone: false, system: None, use_system: true };
                    let res = AudioCapture::start(&cfg, &clock, tx).map(SysMeter::Audio);
                    if res.is_ok() {
                        let ctx2 = ctx.clone();
                        std::thread::spawn(move || {
                            while let Ok(b) = rx.recv() {
                                set_peak(&level, screencraft_audio::mix::peak(&b.data));
                                ctx2.request_repaint_after(std::time::Duration::from_millis(40));
                            }
                        });
                    }
                    res
                };
                #[cfg(target_os = "macos")]
                let r = {
                    let (tx, rx) = std::sync::mpsc::sync_channel::<Event>(4);
                    let mut cap = create_capturer();
                    let req = CaptureRequest { target: Target::Display(None), fps: 5, max_long_side: Some(64), system_audio: true };
                    match cap.start(&req, Clock::start(), tx) {
                        Ok(info) if info.provides_system_audio => {
                            let ctx2 = ctx.clone();
                            std::thread::spawn(move || {
                                while let Ok(ev) = rx.recv() {
                                    match ev {
                                        Event::SystemAudio(b) => {
                                            set_peak(&level, screencraft_audio::mix::peak(&b.data));
                                            ctx2.request_repaint_after(std::time::Duration::from_millis(40));
                                        }
                                        Event::Frame(_) => {}
                                        Event::Ended(_) => break,
                                    }
                                }
                            });
                            Ok(SysMeter::Capturer(cap))
                        }
                        Ok(_) => {
                            cap.stop();
                            Err("no system-audio meter on this platform".to_string())
                        }
                        Err(e) => Err(e),
                    }
                };
                *slot.lock().unwrap() = Some(r);
                flag.store(false, Ordering::SeqCst);
                ctx.request_repaint();
            });
        }
    }

    /// The current camera picture as a small square texture for the bar.
    pub fn camera_texture(&mut self) -> Option<egui::TextureId> {
        let f = self.hub.latest()?;
        let ptr = Arc::as_ptr(&f) as usize;
        if ptr != self.last_frame || self.tex.is_none() {
            self.last_frame = ptr;
            const S: usize = 96;
            // centre crop to a square, then sample down
            let side = f.width.min(f.height) as usize;
            let (ox, oy) = ((f.width as usize - side) / 2, (f.height as usize - side) / 2);
            let mut px = Vec::with_capacity(S * S);
            for y in 0..S {
                let sy = oy + y * side / S;
                for x in 0..S {
                    let sx = ox + x * side / S;
                    let o = (sy * f.width as usize + sx) * 3;
                    px.push(egui::Color32::from_rgb(f.rgb[o], f.rgb[o + 1], f.rgb[o + 2]));
                }
            }
            let img = egui::ColorImage::new([S, S], px);
            match &mut self.tex {
                Some(t) => t.set(img, egui::TextureOptions::LINEAR),
                None => self.tex = Some(self.ctx.load_texture("camera-preview", img, egui::TextureOptions::LINEAR)),
            }
        }
        self.tex.as_ref().map(|t| t.id())
    }

    /// The current camera picture cropped to match the active aspect ratio for the floating viewfinder.
    pub fn preview_texture(&mut self, vertical: bool) -> Option<(egui::TextureId, u32, u32)> {
        let f = self.hub.latest()?;
        let (cx, cy, cw, ch) = screencraft_capture::camera_capture::crop_for(f.width, f.height, vertical);
        let ptr = Arc::as_ptr(&f) as usize;
        if (ptr, vertical) != self.last_preview || self.preview_tex.is_none() {
            self.last_preview = (ptr, vertical);
            let long = cw.max(ch).max(1) as f32;
            let scale = (480.0 / long).min(1.0);
            let ow = ((cw as f32 * scale).round() as usize).max(2);
            let oh = ((ch as f32 * scale).round() as usize).max(2);
            let mut px = Vec::with_capacity(ow * oh);
            for y in 0..oh {
                let sy = cy as usize + y * ch as usize / oh;
                for x in 0..ow {
                    let sx = cx as usize + x * cw as usize / ow;
                    let o = (sy * f.width as usize + sx) * 3;
                    px.push(egui::Color32::from_rgb(f.rgb[o], f.rgb[o + 1], f.rgb[o + 2]));
                }
            }
            let img = egui::ColorImage::new([ow, oh], px);
            match &mut self.preview_tex {
                Some(t) => t.set(img, egui::TextureOptions::LINEAR),
                None => self.preview_tex = Some(self.ctx.load_texture("camera-viewfinder", img, egui::TextureOptions::LINEAR)),
            }
        }
        self.preview_tex.as_ref().map(|t| (t.id(), cw, ch))
    }
}
