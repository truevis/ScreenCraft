//! The 0.3 inspector controls (sliders, toggles, segmented buttons, tiles ...) painted to match
//! the web UI's CSS pixel for pixel.

use crate::icons;
use crate::theme::*;
use eframe::egui::{self, Align2, Color32, CornerRadius, FontId, Pos2, Rect, Response, Sense, Shape, Stroke, StrokeKind, Ui, Vec2};

pub fn font(size: f32) -> FontId {
    FontId::proportional(size)
}

pub fn text_width(ui: &Ui, text: &str, size: f32) -> f32 {
    ui.painter().layout_no_wrap(text.to_string(), font(size), Color32::WHITE).size().x
}

/// `text` shortened with an ellipsis so it fits `max_w` (CSS `text-overflow: ellipsis`).
pub fn fit_text(ui: &Ui, text: &str, size: f32, max_w: f32) -> String {
    if text_width(ui, text, size) <= max_w {
        return text.to_string();
    }
    let offsets: Vec<usize> = text.char_indices().map(|(i, _)| i).collect();
    let (mut lo, mut hi) = (0usize, offsets.len());
    while lo < hi {
        let mid = (lo + hi).div_ceil(2);
        let cand = format!("{}…", &text[..offsets[mid - 1]]);
        if text_width(ui, &cand, size) <= max_w {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    if lo == 0 {
        "…".to_string()
    } else {
        format!("{}…", text[..offsets[lo - 1]].trim_end())
    }
}

/// Text with CSS-style letter spacing; returns its width.
pub fn spaced_text(ui: &Ui, pos: Pos2, text: &str, size: f32, color: Color32, spacing: f32) -> f32 {
    if text.is_empty() {
        return 0.0;
    }
    let mut job = egui::text::LayoutJob::default();
    job.append(
        text,
        0.0,
        egui::TextFormat {
            font_id: font(size),
            color,
            extra_letter_spacing: spacing,
            ..Default::default()
        },
    );
    let g = ui.painter().layout_job(job);
    let w = g.size().x;
    ui.painter().galley(pos, g, color);
    w
}

/// Section heading inside a panel (`#panel h3`).
pub fn h3(ui: &mut Ui, text: &str, first: bool) {
    ui.add_space(if first { 2.0 } else { 18.0 });
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 13.0), Sense::hover());
    spaced_text(ui, rect.min, &text.to_uppercase(), 10.5, MUTED, 1.26);
    ui.add_space(10.0 - ui.spacing().item_spacing.y);
}

/// Section heading with a compact `?` help icon tooltip.
pub fn h3_help(ui: &mut Ui, text: &str, first: bool, help: &str) {
    ui.add_space(if first { 2.0 } else { 18.0 });
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 15.0), Sense::hover());
    let w = spaced_text(ui, Pos2::new(rect.min.x, rect.min.y + 1.0), &text.to_uppercase(), 10.5, MUTED, 1.26);
    let icon_r = Rect::from_center_size(Pos2::new(rect.min.x + w + 13.0, rect.center().y), Vec2::splat(14.0));
    let resp = ui.interact(icon_r.expand(2.0), ui.id().with(("h3-help", text)), Sense::hover());
    let hov = resp.hovered();
    ui.painter().circle_filled(icon_r.center(), 7.0, if hov { RAISE } else { PANEL_2 });
    ui.painter().circle_stroke(icon_r.center(), 7.0, Stroke::new(1.0, LINE_2));
    ui.painter().text(icon_r.center(), Align2::CENTER_CENTER, "?", font(10.0), if hov { Color32::WHITE } else { MUTED });
    resp.on_hover_cursor(egui::CursorIcon::Help).on_hover_text(help);
    ui.add_space(10.0 - ui.spacing().item_spacing.y);
}

/// Wrapped muted text (`.hint` / `.muted`).
pub fn hint(ui: &mut Ui, text: &str) {
    ui.add_space(2.0);
    ui.add(egui::Label::new(egui::RichText::new(text).size(12.0).color(MUTED)).wrap());
    ui.add_space(2.0);
}

pub fn muted(ui: &mut Ui, text: &str) {
    ui.add(egui::Label::new(egui::RichText::new(text).size(13.0).color(MUTED)).wrap());
}

/// `.field > label`: a label on the left and an accent value on the right.
fn field_label(ui: &mut Ui, label: &str, value: Option<&str>) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 16.0), Sense::hover());
    ui.painter().text(rect.left_center(), Align2::LEFT_CENTER, label, font(13.0), LABEL);
    if let Some(v) = value {
        ui.painter().text(rect.right_center(), Align2::RIGHT_CENTER, v, font(13.0), ACCENT_2);
    }
}

fn parse_slider_input(raw: &str, max: f64, fmt_str: &str) -> Option<f64> {
    let trimmed = raw.trim();
    let has_pct = trimmed.ends_with('%');
    let num_str = trimmed.trim_end_matches(|c: char| !c.is_ascii_digit() && c != '.' && c != '-' && c != '+').trim();
    let val: f64 = num_str.parse().ok()?;
    if (has_pct || fmt_str.contains('%')) && max <= 1.5 {
        Some(val / 100.0)
    } else {
        Some(val)
    }
}

/// `<input type=range>` with accent progress fill, click-to-type value label, keyboard arrows,
/// and optional double-click reset. Returns true when the value changed.
pub fn slider(ui: &mut Ui, label: &str, v: &mut f64, min: f64, max: f64, step: f64, fmt: impl Fn(f64) -> String) -> bool {
    slider_default(ui, label, v, min, max, step, None, fmt)
}

pub fn slider_default(ui: &mut Ui, label: &str, v: &mut f64, min: f64, max: f64, step: f64, default: Option<f64>, fmt: impl Fn(f64) -> String) -> bool {
    ui.add_space(6.0);
    let mut changed = false;
    let formatted = fmt(*v);
    let edit_id = ui.id().with(("slider-edit", label));
    let mut editing: Option<String> = ui.ctx().memory_mut(|m| m.data.get_temp(edit_id));
    {
        let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 18.0), Sense::hover());
        ui.painter().text(rect.left_center(), Align2::LEFT_CENTER, label, font(13.0), LABEL);
        let vw = (text_width(ui, &formatted, 13.0) + 10.0).max(48.0).min(rect.width() * 0.5);
        let val_rect = Rect::from_min_max(Pos2::new(rect.max.x - vw, rect.min.y), rect.max);
        if let Some(buf) = editing.as_mut() {
            let mut child = ui.new_child(egui::UiBuilder::new().max_rect(val_rect));
            let te = child.add_sized(val_rect.size(), egui::TextEdit::singleline(buf).font(font(12.5)).horizontal_align(egui::Align::RIGHT).margin(egui::Margin::symmetric(4, 1)));
            te.request_focus();
            let esc = ui.ctx().input(|i| i.key_pressed(egui::Key::Escape));
            let enter = ui.ctx().input(|i| i.key_pressed(egui::Key::Enter));
            if esc {
                editing = None;
            } else if enter || te.lost_focus() {
                if let Some(mut nv) = parse_slider_input(buf, max, &formatted) {
                    if step > 0.0 {
                        nv = (nv / step).round() * step;
                    }
                    nv = nv.clamp(min, max);
                    if (nv - *v).abs() > f64::EPSILON {
                        *v = nv;
                        changed = true;
                    }
                }
                editing = None;
            }
        } else {
            let vr = ui.interact(val_rect, edit_id.with("val"), Sense::click());
            let col = if vr.hovered() { Color32::WHITE } else { ACCENT_2 };
            ui.painter().text(rect.right_center(), Align2::RIGHT_CENTER, &formatted, font(13.0), col);
            if vr.on_hover_text("Click to enter exact value").on_hover_cursor(egui::CursorIcon::Text).clicked() {
                let initial = if formatted.contains('%') && max <= 1.5 {
                    format!("{}", (*v * 100.0).round())
                } else {
                    format!("{}", (*v * 1000.0).round() / 1000.0)
                };
                editing = Some(initial);
            }
        }
    }
    ui.ctx().memory_mut(|m| match editing {
        Some(s) => {
            m.data.insert_temp(edit_id, s);
        }
        None => {
            m.data.remove::<String>(edit_id);
        }
    });

    ui.add_space(1.0);
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 18.0), Sense::click_and_drag());
    let (tw, half) = (14.0, 7.0);
    let track = Rect::from_min_max(Pos2::new(rect.min.x, rect.center().y - 2.0), Pos2::new(rect.max.x, rect.center().y + 2.0));
    if resp.double_clicked() {
        if let Some(def) = default {
            let nv = def.clamp(min, max);
            if (nv - *v).abs() > f64::EPSILON {
                *v = nv;
                changed = true;
            }
        }
    } else if resp.dragged() || resp.clicked() || resp.drag_started() {
        resp.request_focus();
        if let Some(p) = resp.interact_pointer_pos() {
            let t = ((p.x - rect.min.x - half) / (rect.width() - tw)).clamp(0.0, 1.0) as f64;
            let mut nv = min + (max - min) * t;
            if step > 0.0 {
                nv = (nv / step).round() * step;
            }
            nv = nv.clamp(min, max);
            if (nv - *v).abs() > f64::EPSILON {
                *v = nv;
                changed = true;
            }
        }
    }
    if resp.has_focus() {
        let inc = if step > 0.0 { step } else { (max - min) / 50.0 };
        let mult = if ui.ctx().input(|i| i.modifiers.shift) { 10.0 } else { 1.0 };
        let delta = ui.ctx().input(|i| {
            if i.key_pressed(egui::Key::ArrowRight) || i.key_pressed(egui::Key::ArrowUp) {
                Some(inc * mult)
            } else if i.key_pressed(egui::Key::ArrowLeft) || i.key_pressed(egui::Key::ArrowDown) {
                Some(-inc * mult)
            } else {
                None
            }
        });
        if let Some(d) = delta {
            let nv = (*v + d).clamp(min, max);
            if (nv - *v).abs() > f64::EPSILON {
                *v = nv;
                changed = true;
            }
        }
        if let Some(def) = default {
            if ui.ctx().input(|i| i.key_pressed(egui::Key::Backspace) || i.key_pressed(egui::Key::Delete)) {
                let nv = def.clamp(min, max);
                if (nv - *v).abs() > f64::EPSILON {
                    *v = nv;
                    changed = true;
                }
            }
        }
    }
    let t = if max > min { ((*v - min) / (max - min)).clamp(0.0, 1.0) as f32 } else { 0.0 };
    let cx = rect.min.x + half + t * (rect.width() - tw);
    ui.painter().rect_filled(track, CornerRadius::same(2), LINE_2);
    if cx > track.min.x {
        ui.painter().rect_filled(Rect::from_min_max(track.min, Pos2::new(cx, track.max.y)), CornerRadius::same(2), ACCENT);
    }
    let thumb = Rect::from_center_size(Pos2::new(cx, rect.center().y), Vec2::splat(tw));
    let col = if resp.hovered() || resp.dragged() || resp.has_focus() { Color32::WHITE } else { ACCENT_2 };
    ui.painter().rect_filled(thumb.expand(1.0), CornerRadius::same(5), ACCENT_2);
    ui.painter().rect_filled(thumb, CornerRadius::same(4), BG);
    ui.painter().rect_filled(thumb.shrink(2.0), CornerRadius::same(3), col);
    let resp = if default.is_some() {
        resp.on_hover_text("Drag, click value to type, or double-click to reset")
    } else {
        resp
    };
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    ui.add_space(6.0);
    changed
}

fn paint_toggle(painter: &egui::Painter, rect: Rect, on: bool) {
    painter.rect_filled(rect, CornerRadius::same(6), if on { ACCENT } else { LINE_2 });
    let kx = rect.min.x + 3.0 + if on { 16.0 } else { 0.0 };
    painter.rect_filled(Rect::from_min_size(Pos2::new(kx, rect.min.y + 3.0), Vec2::splat(14.0)), CornerRadius::same(4), Color32::WHITE);
}

/// Bare 36x20 switch toggle, returning true when clicked.
pub fn toggle(ui: &mut Ui, v: &mut bool) -> bool {
    let (r, resp) = ui.allocate_exact_size(Vec2::new(36.0, 20.0), Sense::click());
    paint_toggle(ui.painter(), r, *v);
    if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
        *v = !*v;
        return true;
    }
    false
}

/// `.row-t` with a 36x20 toggle.
pub fn toggle_row(ui: &mut Ui, label: &str, hint_text: Option<&str>, v: &mut bool) -> bool {
    ui.add_space(6.0);
    let w = ui.available_width();
    let label_w = w - 36.0 - 12.0;
    let galley = ui.painter().layout(label.to_string(), font(13.0), TEXT, label_w);
    let hint_g = hint_text.map(|h| ui.painter().layout(h.to_string(), font(12.0), MUTED, label_w));
    let h = galley.size().y + hint_g.as_ref().map(|g| g.size().y + 2.0).unwrap_or(0.0);
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(w, h.max(20.0)), Sense::click());
    let top = rect.min;
    ui.painter().galley(top, galley.clone(), TEXT);
    if let Some(g) = hint_g {
        ui.painter().galley(Pos2::new(top.x, top.y + galley.size().y + 2.0), g, MUTED);
    }
    let tg = Rect::from_min_size(Pos2::new(rect.max.x - 36.0, rect.center().y - 10.0), Vec2::new(36.0, 20.0));
    paint_toggle(ui.painter(), tg, *v);
    ui.add_space(6.0);
    if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
        *v = !*v;
        return true;
    }
    false
}

struct Cell<'a, T> {
    value: T,
    label: &'a str,
    w: f32,
}

/// `.seg`: joined buttons in a rounded 1px box, the current one filled with the accent.
pub fn segmented<T: PartialEq + Clone>(ui: &mut Ui, label: Option<&str>, options: &[(T, &str)], cur: &mut T) -> bool {
    if let Some(l) = label {
        ui.add_space(6.0);
        field_label(ui, l, None);
        ui.add_space(1.0);
    }
    let avail = ui.available_width();
    let cells: Vec<Cell<T>> =
        options.iter().map(|(v, l)| Cell { value: v.clone(), label: l, w: text_width(ui, l, 13.0) + 24.0 + 1.0 }).collect();
    // wrap into rows
    let mut rows: Vec<Vec<usize>> = vec![vec![]];
    let mut x = 0.0;
    for (i, c) in cells.iter().enumerate() {
        if x + c.w > avail && !rows.last().unwrap().is_empty() {
            rows.push(vec![]);
            x = 0.0;
        }
        rows.last_mut().unwrap().push(i);
        x += c.w;
    }
    let row_h = 29.0;
    let used_w = rows.iter().map(|r| r.iter().map(|&i| cells[i].w).sum::<f32>()).fold(0.0, f32::max).min(avail);
    let (rect, _) = ui.allocate_exact_size(Vec2::new(used_w, row_h * rows.len() as f32), Sense::hover());
    let mut changed = false;
    let nrows = rows.len();
    let mut hit: Vec<(Rect, usize, bool, bool, usize)> = Vec::new();
    for (ri, row) in rows.iter().enumerate() {
        let mut x = rect.min.x;
        for (k, &i) in row.iter().enumerate() {
            let r = Rect::from_min_size(Pos2::new(x, rect.min.y + ri as f32 * row_h), Vec2::new(cells[i].w, row_h));
            hit.push((r, i, k == 0, k == row.len() - 1, ri));
            x += cells[i].w;
        }
    }
    for (r, i, first, last, ri) in hit {
        let resp = ui.interact(r, ui.id().with(("seg", i, label, r.min.x.round() as i32, r.min.y.round() as i32)), Sense::click());
        let on = cells[i].value == *cur;
        let cr = CornerRadius {
            nw: if first && ri == 0 { 7 } else { 0 },
            ne: if last && ri == 0 { 7 } else { 0 },
            sw: if first && ri == nrows - 1 { 7 } else { 0 },
            se: if last && ri == nrows - 1 { 7 } else { 0 },
        };
        let inner = r.shrink(1.0);
        if on {
            ui.painter().rect_filled(inner, cr, ACCENT);
        } else if resp.hovered() {
            ui.painter().rect_filled(inner, cr, PANEL_2);
        }
        ui.painter().text(r.center() - Vec2::new(0.5, 0.0), Align2::CENTER_CENTER, cells[i].label, font(13.0), if on { Color32::WHITE } else { LABEL });
        if !last {
            ui.painter().line_segment([Pos2::new(r.max.x - 0.5, r.min.y + 1.0), Pos2::new(r.max.x - 0.5, r.max.y - 1.0)], Stroke::new(1.0, LINE_2));
        }
        if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() && !on {
            *cur = cells[i].value.clone();
            changed = true;
        }
    }
    ui.painter().rect_stroke(rect, CornerRadius::same(8), Stroke::new(1.0, LINE_2), StrokeKind::Inside);
    ui.add_space(6.0);
    changed
}

/// Colour row: label left, 44x30 swatch right that opens the picker.
pub fn color_row(ui: &mut Ui, label: &str, hex: &mut String) -> bool {
    ui.add_space(6.0);
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(label).size(13.0).color(TEXT));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let mut c = parse_hex(hex);
            ui.spacing_mut().interact_size = Vec2::new(44.0, 30.0);
            if egui::color_picker::color_edit_button_srgba(ui, &mut c, egui::color_picker::Alpha::Opaque).changed() {
                *hex = to_hex(c);
                changed = true;
            }
        });
    });
    ui.add_space(6.0);
    changed
}

fn dashed_rect(ui: &Ui, r: Rect, stroke: Stroke) {
    let corners = [r.left_top(), r.right_top(), r.right_bottom(), r.left_bottom(), r.left_top()];
    for w in corners.windows(2) {
        ui.painter().extend(Shape::dashed_line(&[w[0], w[1]], stroke, 4.0, 3.0));
    }
}

/// `.btn-line`: a full-width dashed button.
pub fn btn_line(ui: &mut Ui, text: &str, danger: bool) -> Response {
    btn_line_w(ui, text, danger, ui.available_width())
}

pub fn btn_line_w(ui: &mut Ui, text: &str, danger: bool, w: f32) -> Response {
    ui.add_space(2.0);
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(w, 33.0), Sense::click());
    let hov = resp.hovered();
    ui.painter().rect_filled(rect, CornerRadius::same(7), if hov { RAISE } else { PANEL_2 });
    if hov {
        ui.painter().rect_stroke(rect, CornerRadius::same(7), Stroke::new(1.0, LINE_2), StrokeKind::Inside);
    } else {
        dashed_rect(ui, rect.shrink(0.5), Stroke::new(1.0, LINE_2));
    }
    ui.painter().text(rect.center(), Align2::CENTER_CENTER, text, font(13.0), if danger { rgb_danger() } else { rgb_btn() });
    ui.add_space(2.0);
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// A full-width dashed button with an integrated progress bar fill.
pub fn btn_progress(ui: &mut Ui, text: &str, frac: Option<f32>, danger: bool) -> Response {
    ui.add_space(2.0);
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 33.0), Sense::click());
    let hov = resp.hovered();
    ui.painter().rect_filled(rect, CornerRadius::same(7), if hov { RAISE } else { PANEL_2 });
    if let Some(f) = frac {
        let f = f.clamp(0.0, 1.0);
        let fill_w = rect.width() * f;
        if fill_w > 0.5 {
            let fill_rect = Rect::from_min_size(rect.min, Vec2::new(fill_w, rect.height()));
            ui.painter().rect_filled(fill_rect, CornerRadius::same(7), alpha(ACCENT, 0.28));
            let bar_x1 = (rect.min.x + fill_w).min(rect.max.x - 2.0);
            let bar_x0 = rect.min.x + 2.0;
            if bar_x1 > bar_x0 {
                let bar = Rect::from_min_max(Pos2::new(bar_x0, rect.max.y - 2.5), Pos2::new(bar_x1, rect.max.y));
                ui.painter().rect_filled(bar, CornerRadius::same(2), ACCENT_2);
            }
        }
    } else {
        let t = ui.ctx().input(|i| i.time) as f32;
        let bar_w = rect.width() * 0.30;
        let phase = (t * 0.75) % 1.0;
        let start = rect.min.x - bar_w + phase * (rect.width() + bar_w);
        let x0 = start.max(rect.min.x + 1.0);
        let x1 = (start + bar_w).min(rect.max.x - 1.0);
        if x1 > x0 {
            let slide = Rect::from_min_max(Pos2::new(x0, rect.min.y + 1.0), Pos2::new(x1, rect.max.y - 1.0));
            ui.painter().rect_filled(slide, CornerRadius::same(6), alpha(ACCENT, 0.25));
        }
        ui.ctx().request_repaint();
    }
    if hov {
        ui.painter().rect_stroke(rect, CornerRadius::same(7), Stroke::new(1.0, LINE_2), StrokeKind::Inside);
    } else {
        dashed_rect(ui, rect.shrink(0.5), Stroke::new(1.0, LINE_2));
    }
    ui.painter().text(rect.center(), Align2::CENTER_CENTER, text, font(13.0), if danger { rgb_danger() } else { rgb_btn() });
    ui.add_space(2.0);
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

fn rgb_danger() -> Color32 {
    Color32::from_rgb(0xff, 0x8a, 0x97)
}
fn rgb_btn() -> Color32 {
    Color32::from_rgb(0xcd, 0xd5, 0xe5)
}

/// `.pill`: a bordered button, optionally with a leading icon and a trailing chevron.
pub fn pill(ui: &mut Ui, icon: Option<&str>, text: &str, on: bool, small: bool, chevron: bool, color: Option<Color32>) -> Response {
    let size = if small { 12.0 } else { 13.0 };
    let (px, py) = if small { (9.0, 5.0) } else { (11.0, 7.0) };
    let tw = text_width(ui, text, size);
    let mut w = px * 2.0 + tw;
    if icon.is_some() {
        w += 18.0 + 6.0;
    }
    if chevron {
        w += 13.0 + 6.0;
    }
    let h = size * 1.25 + py * 2.0 + 2.0;
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(w, h.max(if small { 26.0 } else { 33.0 })), Sense::click());
    let hov = resp.hovered();
    let (bg, border) = if on { (alpha(ACCENT, 0.25), ACCENT) } else if hov { (RAISE, LINE_2) } else { (PANEL_2, LINE) };
    ui.painter().rect_filled(rect, CornerRadius::same(7), bg);
    ui.painter().rect_stroke(rect, CornerRadius::same(7), Stroke::new(1.0, border), StrokeKind::Inside);
    let fg = color.unwrap_or(Color32::from_rgb(0xd3, 0xdb, 0xea));
    let mut x = rect.min.x + px;
    if let Some(i) = icon {
        icons::paint(ui, i, Rect::from_center_size(Pos2::new(x + 9.0, rect.center().y), Vec2::splat(18.0)), fg);
        x += 24.0;
    }
    ui.painter().text(Pos2::new(x, rect.center().y), Align2::LEFT_CENTER, text, font(size), fg);
    if chevron {
        icons::paint(ui, "chev", Rect::from_center_size(Pos2::new(rect.max.x - px - 6.5, rect.center().y), Vec2::splat(13.0)), fg);
    }
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// `.primary`: the accent call to action.
pub fn primary(ui: &mut Ui, icon: Option<&str>, text: &str) -> Response {
    let tw = text_width(ui, text, 13.0);
    let w = 30.0 + tw + if icon.is_some() { 25.0 } else { 0.0 };
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(w, 34.0), Sense::click());
    ui.painter().rect_filled(rect, CornerRadius::same(7), if resp.hovered() { ACCENT_2 } else { ACCENT });
    ui.painter().rect_stroke(rect, CornerRadius::same(7), Stroke::new(1.0, Color32::from_white_alpha(41)), StrokeKind::Inside);
    let mut x = rect.min.x + 15.0;
    if let Some(i) = icon {
        icons::paint(ui, i, Rect::from_center_size(Pos2::new(x + 9.0, rect.center().y), Vec2::splat(18.0)), Color32::WHITE);
        x += 25.0;
    }
    ui.painter().text(Pos2::new(x, rect.center().y), Align2::LEFT_CENTER, text, font(13.0), Color32::WHITE);
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Destructive call to action (red fill, white label).
pub fn danger(ui: &mut Ui, icon: Option<&str>, text: &str) -> Response {
    let tw = text_width(ui, text, 13.0);
    let w = 30.0 + tw + if icon.is_some() { 25.0 } else { 0.0 };
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(w, 34.0), Sense::click());
    let bg = if resp.hovered() { mix(DANGER, Color32::WHITE, 0.14) } else { DANGER };
    ui.painter().rect_filled(rect, CornerRadius::same(7), bg);
    ui.painter().rect_stroke(rect, CornerRadius::same(7), Stroke::new(1.0, Color32::from_white_alpha(41)), StrokeKind::Inside);
    let mut x = rect.min.x + 15.0;
    if let Some(i) = icon {
        icons::paint(ui, i, Rect::from_center_size(Pos2::new(x + 9.0, rect.center().y), Vec2::splat(18.0)), Color32::WHITE);
        x += 25.0;
    }
    ui.painter().text(Pos2::new(x, rect.center().y), Align2::LEFT_CENTER, text, font(13.0), Color32::WHITE);
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// `.icon-btn`: 32x32 ghost button.
pub fn icon_btn(ui: &mut Ui, name: &str, tip: &str) -> Response {
    icon_btn_sized(ui, name, tip, 32.0, 18.0, true)
}

pub fn icon_btn_sized(ui: &mut Ui, name: &str, tip: &str, box_px: f32, icon_px: f32, enabled: bool) -> Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(box_px), Sense::click());
    let hov = resp.hovered() && enabled;
    if hov {
        ui.painter().rect_filled(rect, CornerRadius::same(7), RAISE);
    }
    let col = if !enabled { alpha(ICON, 0.4) } else if hov { Color32::WHITE } else { ICON };
    icons::paint(ui, name, Rect::from_center_size(rect.center(), Vec2::splat(icon_px)), col);
    let r = if tip.is_empty() { resp } else { resp.on_hover_text(tip) };
    if enabled {
        r.on_hover_cursor(egui::CursorIcon::PointingHand)
    } else {
        r
    }
}

/// `.add-tile`: icon over a label, in a bordered tile.
pub fn add_tile(ui: &mut Ui, icon: Option<&str>, label: &str, w: f32) -> Response {
    let h = if icon.is_some() { 62.0 } else { 34.0 };
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(w, h), Sense::click());
    let hov = resp.hovered();
    ui.painter().rect_filled(rect, CornerRadius::same(8), if hov { RAISE } else { PANEL_2 });
    ui.painter().rect_stroke(rect, CornerRadius::same(8), Stroke::new(1.0, if hov { ACCENT_2 } else { LINE_2 }), StrokeKind::Inside);
    let fg = if hov { Color32::WHITE } else { Color32::from_rgb(0xcd, 0xd6, 0xe8) };
    if let Some(i) = icon {
        icons::paint(ui, i, Rect::from_center_size(Pos2::new(rect.center().x, rect.min.y + 20.0), Vec2::splat(20.0)), fg);
        ui.painter().text(Pos2::new(rect.center().x, rect.max.y - 14.0), Align2::CENTER_CENTER, label, font(11.0), fg);
    } else {
        ui.painter().text(rect.center(), Align2::CENTER_CENTER, label, font(11.0), fg);
    }
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// A grid of `n` equal columns, `gap` apart, calling `item(ui, index, width)` per cell.
pub fn grid(ui: &mut Ui, n: usize, count: usize, gap: f32, row_h: f32, mut item: impl FnMut(&mut Ui, usize, Rect)) {
    let w = ui.available_width();
    let cw = (w - gap * (n as f32 - 1.0)) / n as f32;
    let rows = count.div_ceil(n);
    let (rect, _) = ui.allocate_exact_size(Vec2::new(w, rows as f32 * row_h + gap * (rows.max(1) as f32 - 1.0)), Sense::hover());
    for i in 0..count {
        let (r, c) = (i / n, i % n);
        let cell = Rect::from_min_size(Pos2::new(rect.min.x + c as f32 * (cw + gap), rect.min.y + r as f32 * (row_h + gap)), Vec2::new(cw, row_h));
        item(ui, i, cell);
    }
}

pub fn cell_response(ui: &mut Ui, rect: Rect, id: impl std::hash::Hash + std::fmt::Debug) -> Response {
    ui.interact(rect, ui.id().with(id), Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// The 3x3 anchor grid (`.pos-grid`). `cur` is the current (x, y) in 0..1.
pub fn pos_grid(ui: &mut Ui, label: &str, cur: (f64, f64)) -> Option<(f64, f64)> {
    ui.add_space(6.0);
    field_label(ui, label, None);
    ui.add_space(6.0);
    let spots: [(f64, f64, &str); 9] = [
        (0.0, 0.0, "↖"),
        (0.5, 0.0, "↑"),
        (1.0, 0.0, "↗"),
        (0.0, 0.5, "←"),
        (0.5, 0.5, "•"),
        (1.0, 0.5, "→"),
        (0.0, 1.0, "↙"),
        (0.5, 1.0, "↓"),
        (1.0, 1.0, "↘"),
    ];
    let mut out = None;
    let w = 168.0_f32.min(ui.available_width());
    ui.allocate_ui(Vec2::new(w, 0.0), |ui| {
        grid(ui, 3, 9, 4.0, 29.0, |ui, i, cell| {
            let (x, y, name) = spots[i];
            let on = (cur.0 - x).abs() < 0.01 && (cur.1 - y).abs() < 0.01;
            let resp = cell_response(ui, cell, ("pos", label, i));
            ui.painter().rect_filled(cell, CornerRadius::same(6), if on { ACCENT } else if resp.hovered() { RAISE } else { PANEL_2 });
            ui.painter().rect_stroke(cell, CornerRadius::same(6), Stroke::new(1.0, if on { ACCENT } else { LINE_2 }), StrokeKind::Inside);
            ui.painter().text(cell.center(), Align2::CENTER_CENTER, name, font(13.0), if on { Color32::WHITE } else { Color32::from_rgb(0xcd, 0xd6, 0xe8) });
            if resp.clicked() {
                out = Some((x, y));
            }
        });
    });
    ui.add_space(6.0);
    out
}

/// A `<select>`: returns the newly chosen index.
pub fn select(ui: &mut Ui, id: &str, current: &str, items: &[&str], min_w: f32) -> Option<usize> {
    let mut out = None;
    egui::ComboBox::from_id_salt(id).width(min_w).selected_text(egui::RichText::new(current).size(13.0)).show_ui(ui, |ui| {
        for (i, it) in items.iter().enumerate() {
            if ui.selectable_label(*it == current, *it).clicked() {
                out = Some(i);
            }
        }
    });
    out
}

/// Dim the backdrop and show a centered frameless modal window. Returns `true` when the backdrop
/// itself was clicked.
pub fn modal(ctx: &egui::Context, id: &str, width: f32, fill: Color32, margin: egui::Margin, dim_alpha: u8, content: impl FnOnce(&mut Ui)) -> bool {
    let screen = ctx.content_rect();
    let mut outside = false;
    egui::Area::new(egui::Id::new((id, "dim"))).order(egui::Order::Middle).fixed_pos(screen.min).show(ctx, |ui| {
        let r = ui.allocate_rect(screen, Sense::click());
        ui.painter().rect_filled(screen, 0.0, Color32::from_rgba_unmultiplied(4, 7, 12, dim_alpha));
        if r.clicked() {
            outside = true;
        }
    });
    egui::Window::new(id)
        .title_bar(false)
        .collapsible(false)
        .resizable(false)
        .order(egui::Order::Foreground)
        .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
        .fixed_size(Vec2::new(width, 0.0))
        .frame(
            egui::Frame::new()
                .fill(fill)
                .stroke(Stroke::new(1.0, LINE_2))
                .corner_radius(CornerRadius::same(12))
                .inner_margin(margin)
                .shadow(egui::Shadow { offset: [0, 24], blur: 70, spread: 0, color: Color32::from_black_alpha(140) }),
        )
        .show(ctx, content);
    outside
}
