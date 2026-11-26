use super::*;
use crate::editor::Sel;
use screencraft_core::project::{ZoomKind, ZoomRegion};

fn zslider(
    st: &mut EditorState,
    ui: &mut Ui,
    ids: &[String],
    key: &str,
    label: &str,
    cur: f64,
    range: (f64, f64, f64),
    fmt: impl Fn(f64) -> String,
    set: impl Fn(&mut ZoomRegion, f64),
) {
    let mut v = cur;
    if kit::slider(ui, label, &mut v, range.0, range.1, range.2, fmt) {
        st.edit_keyed(Some(key), |p| {
            for r in p.scene.zooms.iter_mut().filter(|r| ids.contains(&r.id)) {
                set(r, v);
            }
        });
    }
}

fn ztoggle(st: &mut EditorState, ui: &mut Ui, ids: &[String], label: &str, hint: Option<&str>, cur: bool, set: impl Fn(&mut ZoomRegion, bool)) {
    let mut v = cur;
    if kit::toggle_row(ui, label, hint, &mut v) {
        st.edit(|p| {
            for r in p.scene.zooms.iter_mut().filter(|r| ids.contains(&r.id)) {
                set(r, v);
            }
        });
    }
}

pub fn show(st: &mut EditorState, ui: &mut Ui) {
    let zoom = if let Sel::Zoom(id) = &st.sel { st.project.scene.zooms.iter().find(|z| &z.id == id).cloned() } else { None };
    let Some(z) = zoom else {
        kit::h3_help(ui, "Zoom", true, "Zooms are created automatically from your clicks and typing. Select a zoom block on the timeline to edit it, or add one at the playhead.");
        let an = st.project.config.animations.clone();
        slider(st, ui, "zl-default", "Default zoom level", an.default_zoom, (1.2, 4.0, 0.1), |v| format!("{v:.1}×"), |p, v| p.config.animations.default_zoom = v);
        slider(st, ui, "ease-in-default", "Default ease in", an.ease_in_ms / 1000.0, (0.0, 1.5, 0.05), sec2, |p, v| p.config.animations.ease_in_ms = v * 1000.0);
        slider(st, ui, "ease-out-default", "Default ease out", an.ease_out_ms / 1000.0, (0.0, 1.5, 0.05), sec2, |p, v| p.config.animations.ease_out_ms = v * 1000.0);
        if kit::btn_line(ui, "Add zoom at playhead", false).clicked() {
            st.add_zoom_at_playhead();
        }
        if kit::btn_line(ui, "Regenerate from clicks & typing", false).clicked() {
            st.auto_zoom();
        }
        if !st.project.scene.zooms.is_empty() && kit::btn_line(ui, "Select all zooms (Ctrl+A)", false).clicked() {
            st.select_all_zooms();
        }
        return;
    };
    let ids = st.selected_zoom_ids();
    let n = ids.len();
    let title = if n > 1 { format!("Zoom regions · {n} selected") } else { "Zoom region".to_string() };
    let help = if n > 1 {
        "Showing the first region’s settings. Changing a control applies it to all selected regions. Ctrl-click to toggle, Shift-click for a range, or Ctrl+A to select all."
    } else {
        "Ctrl-click to toggle zooms, Shift-click to select a range, or Ctrl+A to select all."
    };
    kit::h3_help(ui, &title, true, help);
    let an = st.project.config.animations.clone();
    let k = z.id.clone();
    zslider(st, ui, &ids, &format!("zl-{k}"), "Zoom level", z.zoom, (1.1, 5.0, 0.05), |v| format!("{v:.2}×"), |r, v| r.zoom = v);
    zslider(st, ui, &ids, &format!("ei-{k}"), "Ease in", z.ease_in_ms.unwrap_or(an.ease_in_ms) / 1000.0, (0.0, 1.5, 0.05), sec2, |r, v| r.ease_in_ms = Some(v * 1000.0));
    zslider(st, ui, &ids, &format!("eo-{k}"), "Ease out", z.ease_out_ms.unwrap_or(an.ease_out_ms) / 1000.0, (0.0, 1.5, 0.05), sec2, |r, v| r.ease_out_ms = Some(v * 1000.0));
    let mut kind = z.kind;
    if kit::segmented(ui, Some("Target"), &[(ZoomKind::FollowCursor, "Follow cursor"), (ZoomKind::Manual, "Fixed point")], &mut kind) {
        st.edit(|p| {
            for r in p.scene.zooms.iter_mut().filter(|r| ids.contains(&r.id)) {
                r.kind = kind;
            }
        });
    }
    if z.kind == ZoomKind::Manual {
        kit::hint(ui, "Click or drag on the preview to set the fixed zoom point.");
    } else {
        kit::hint(ui, "The camera keeps the cursor centred and follows the text caret while you type.");
    }
    ztoggle(st, ui, &ids, "Instant animation", Some("Apply the zoom immediately without easing."), z.instant, |r, v| r.instant = v);
    ztoggle(st, ui, &ids, "Disabled", None, z.is_disabled, |r, v| r.is_disabled = v);
    if n < st.project.scene.zooms.len() && kit::btn_line(ui, "Select all zooms", false).clicked() {
        st.select_all_zooms();
    }
    if n > 1 && kit::btn_line(ui, "Clear selection", false).clicked() {
        st.select(Sel::None);
    }
    let label = if n > 1 { format!("Delete {n} zooms") } else { "Delete zoom".to_string() };
    if kit::btn_line(ui, &label, true).clicked() {
        st.delete_selected();
    }
}
