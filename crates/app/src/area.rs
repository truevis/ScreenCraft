//! Area selection and the on-screen outline of what will be recorded: a full-screen dim with a
//! crosshair for dragging out the area, and a click-through frame around the chosen area / window.

use crate::kit;
use crate::theme::*;
use eframe::egui::{self, Align2, Color32, CornerRadius, Pos2, Rect, Stroke, StrokeKind, Vec2, ViewportBuilder, ViewportId};
use screencraft_capture::DisplayInfo;
use screencraft_core::project::Rect as PRect;

pub enum Outcome {
    Pending,
    Picked(PRect),
    Cancelled,
}

pub struct Picker {
    display: Option<DisplayInfo>,
    drag: Option<(Pos2, Pos2)>,
    /// Width / height the drawn area must keep (9:16 for a vertical video).
    ratio: Option<f32>,
    /// Pre-captured desktop screenshot so the picker uses an opaque window instead of a
    /// full-screen transparent viewport (which renders as an opaque gray block on hybrid GPUs).
    snapshot: Option<(u32, u32, Vec<u8>)>,
    tex: Option<egui::TextureHandle>,
}

impl Picker {
    pub fn new(display: Option<DisplayInfo>, ratio: Option<f32>) -> Self {
        let snapshot = screencraft_capture::snapshot_display(display.as_ref().map(|d| d.id));
        Self { display, drag: None, ratio, snapshot, tex: None }
    }

    pub fn display_id(&self) -> Option<u32> {
        self.display.as_ref().map(|d| d.id)
    }

    pub fn show(&mut self, ctx: &egui::Context) -> Outcome {
        let (x, y, w, h) = self.display.as_ref().map(|d| (d.x as f32, d.y as f32, d.width as f32, d.height as f32)).unwrap_or((0.0, 0.0, 1440.0, 900.0));
        let builder = ViewportBuilder::default()
            .with_title("Select area")
            .with_position(Pos2::new(x, y))
            .with_inner_size([w, h])
            .with_decorations(false)
            .with_resizable(false)
            .with_has_shadow(false)
            .with_taskbar(false)
            .with_always_on_top();
        let mut out = Outcome::Pending;
        let mut origin = (x, y);
        let drag = &mut self.drag;
        let ratio = self.ratio;
        let snapshot = &mut self.snapshot;
        let tex = &mut self.tex;
        ctx.show_viewport_immediate(ViewportId::from_hash_of("area-picker"), builder, |ui, _| {
            let ctx = ui.ctx().clone();
            ctx.set_cursor_icon(egui::CursorIcon::Crosshair);
            let full = ui.max_rect();
            // The system may keep the window below the menu bar, so use where it really is.
            if let Some(inner) = ctx.input(|i| i.viewport().inner_rect) {
                origin = (inner.min.x, inner.min.y);
            }
            if tex.is_none() {
                if let Some((sw, sh, rgba)) = snapshot.take() {
                    let img = egui::ColorImage::from_rgba_unmultiplied([sw as usize, sh as usize], &rgba);
                    *tex = Some(ctx.load_texture("area-picker-bg", img, egui::TextureOptions::LINEAR));
                }
            }
            egui::CentralPanel::default().frame(egui::Frame::NONE).show(ui, |ui| {
                let resp = ui.allocate_rect(full, egui::Sense::click_and_drag());
                let painter = ui.painter();
                if let Some(t) = tex.as_ref() {
                    painter.image(t.id(), full, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), Color32::WHITE);
                } else {
                    painter.rect_filled(full, 0.0, Color32::from_rgb(20, 22, 26));
                }
                if resp.drag_started() {
                    if let Some(p) = resp.interact_pointer_pos() {
                        *drag = Some((p, p));
                    }
                }
                if resp.dragged() {
                    if let (Some(d), Some(p)) = (drag.as_mut(), resp.interact_pointer_pos()) {
                        let p = p.clamp(full.min, full.max);
                        d.1 = match ratio {
                            Some(r) => {
                                // keep the shape: take the larger of the two sizes the pointer implies
                                let (dx, dy) = (p.x - d.0.x, p.y - d.0.y);
                                let (w, h) = (dx.abs(), dy.abs());
                                let (mut w2, mut h2) = if h > 0.0 && w / h.max(1.0) > r { (h * r, h) } else { (w, w / r) };
                                // stay on the screen, keeping the shape
                                let room_x = if dx >= 0.0 { full.max.x - d.0.x } else { d.0.x - full.min.x };
                                let room_y = if dy >= 0.0 { full.max.y - d.0.y } else { d.0.y - full.min.y };
                                let k = (room_x / w2.max(1.0)).min(room_y / h2.max(1.0)).min(1.0);
                                w2 *= k;
                                h2 *= k;
                                Pos2::new(d.0.x + w2 * dx.signum(), d.0.y + h2 * dy.signum())
                            }
                            None => p,
                        };
                    }
                }
                let dim = Color32::from_black_alpha(89);
                match *drag {
                    Some((a, b)) => {
                        let sel = Rect::from_two_pos(a, b);
                        // dim everything around the selection
                        for r in [
                            Rect::from_min_max(full.min, Pos2::new(full.max.x, sel.min.y)),
                            Rect::from_min_max(Pos2::new(full.min.x, sel.max.y), full.max),
                            Rect::from_min_max(Pos2::new(full.min.x, sel.min.y), Pos2::new(sel.min.x, sel.max.y)),
                            Rect::from_min_max(Pos2::new(sel.max.x, sel.min.y), Pos2::new(full.max.x, sel.max.y)),
                        ] {
                            painter.rect_filled(r, 0.0, Color32::from_black_alpha(115));
                        }
                        let corners = [sel.left_top(), sel.right_top(), sel.right_bottom(), sel.left_bottom(), sel.left_top()];
                        for w in corners.windows(2) {
                            painter.extend(egui::Shape::dashed_line(&[w[0], w[1]], Stroke::new(2.0, ACCENT_2), 6.0, 4.0));
                        }
                        let label = format!("{} × {}", sel.width().round(), sel.height().round());
                        let tw = kit::text_width(ui, &label, 12.0) + 16.0;
                        let lr = Rect::from_min_size(Pos2::new(sel.min.x, (sel.min.y - 26.0).max(2.0)), Vec2::new(tw, 22.0));
                        painter.rect_filled(lr, CornerRadius::same(4), ACCENT);
                        painter.text(lr.center(), Align2::CENTER_CENTER, label, kit::font(12.0), Color32::WHITE);
                    }
                    None => {
                        painter.rect_filled(full, 0.0, dim);
                    }
                }
                // hint
                let msg = if ratio.is_some() { "Drag to select a vertical (9:16) area · Esc to cancel" } else { "Drag to select the area to record · Esc to cancel" };
                let tw = kit::text_width(ui, msg, 12.0) + 28.0;
                let hr = Rect::from_min_size(Pos2::new(full.center().x - tw / 2.0, full.min.y + 24.0), Vec2::new(tw, 34.0));
                painter.rect_filled(hr, CornerRadius::same(8), PANEL);
                painter.rect_stroke(hr, CornerRadius::same(8), Stroke::new(1.0, LINE_2), StrokeKind::Inside);
                painter.rect_filled(Rect::from_min_size(hr.min, Vec2::new(3.0, hr.height())), CornerRadius { nw: 8, sw: 8, ne: 0, se: 0 }, ACCENT);
                painter.text(hr.center(), Align2::CENTER_CENTER, msg, kit::font(12.0), TEXT);
                if resp.drag_stopped() {
                    if let Some((a, b)) = drag.take() {
                        let sel = Rect::from_two_pos(a, b);
                        if sel.width() >= 24.0 && sel.height() >= 24.0 {
                            out = Outcome::Picked(PRect { x: (origin.0 + sel.min.x) as f64, y: (origin.1 + sel.min.y) as f64, width: sel.width() as f64, height: sel.height() as f64 });
                        }
                    }
                }
            });
            if ctx.input(|i| i.key_pressed(egui::Key::Escape) || i.viewport().close_requested()) {
                out = Outcome::Cancelled;
            }
        });
        out
    }
}

/// An outline as four small opaque click-through edge windows instead of one full-screen
/// transparent window. On hybrid Intel + NVIDIA laptops a full-screen transparent window is
/// drawn as a white, black, or gray block over the screen.
fn draw_edge_frame(ctx: &egui::Context, rect: PRect, thickness: f32, color: Color32, prefix: &str) {
    let (x, y, w, h) = (rect.x as f32, rect.y as f32, rect.width as f32, rect.height as f32);
    let edges = [
        (format!("{prefix}-top"), Pos2::new(x, y), Vec2::new(w, thickness)),
        (format!("{prefix}-bottom"), Pos2::new(x, y + h - thickness), Vec2::new(w, thickness)),
        (format!("{prefix}-left"), Pos2::new(x, y + thickness), Vec2::new(thickness, (h - 2.0 * thickness).max(1.0))),
        (format!("{prefix}-right"), Pos2::new(x + w - thickness, y + thickness), Vec2::new(thickness, (h - 2.0 * thickness).max(1.0))),
    ];
    let bar = |title: &str, pos: Pos2, size: Vec2| {
        ViewportBuilder::default()
            .with_title(title)
            .with_position(pos)
            .with_inner_size(size)
            .with_min_inner_size(size)
            .with_decorations(false)
            .with_resizable(false)
            .with_has_shadow(false)
            .with_taskbar(false)
            .with_mouse_passthrough(true)
            .with_always_on_top()
    };
    for (id, pos, size) in &edges {
        ctx.show_viewport_immediate(ViewportId::from_hash_of(id), bar(id, *pos, *size), |ui, _| {
            ui.painter().rect_filled(ui.max_rect().expand(2.0), 0.0, color);
        });
    }
    // Pin each edge to the display's exact physical pixels (logical window positions are off by
    // a pixel or more at 200% scaling and across monitors with different scaling).
    for (id, pos, size) in &edges {
        screencraft_capture::place_window_titled(id, (pos.x as f64, pos.y as f64, size.x as f64, size.y as f64));
        // keep the border out of the recording (desktop duplication sees every window)
        screencraft_capture::exclude_window_titled(id);
    }
}

/// Visual style for the screen outline overlay.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum FrameStyle {
    Rec,
    Highlight,
    Selection,
}

/// The click-through outline around `rect` (global logical points).
pub fn draw_frame(ctx: &egui::Context, rect: PRect, style: FrameStyle, _displays: &[DisplayInfo]) {
    match style {
        FrameStyle::Rec => draw_edge_frame(ctx, rect, 1.0, Color32::from_rgb(214, 64, 76), "rec"),
        FrameStyle::Selection => draw_edge_frame(ctx, rect, 2.0, ACCENT_2, "sel"),
        FrameStyle::Highlight => draw_edge_frame(ctx, rect, 3.0, ACCENT_2, "hl"),
    }
}
