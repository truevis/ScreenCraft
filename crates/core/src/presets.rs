//! Built-in style presets and partial JSON preset merging shared between the GUI and MCP.

use crate::project::{CameraConfig, CursorConfig, Project, Styles, SubtitleStyle};
use serde_json::{json, Value};

pub fn builtin_presets() -> Vec<(&'static str, Value)> {
    const PAD: f64 = 0.02;
    vec![
        (
            "Clean",
            json!({ "styles": { "paddingRatio": PAD, "cornerRadius": 14, "background": { "kind": "wallpaper", "wallpaper": "sunset" } } }),
        ),
        (
            "Minimal",
            json!({ "styles": { "paddingRatio": PAD, "cornerRadius": 10, "background": { "kind": "color", "color": "#0b0c12" }, "shadow": { "intensity": 0.15 } } }),
        ),
        (
            "Vivid",
            json!({ "styles": { "paddingRatio": PAD, "cornerRadius": 24, "background": { "kind": "gradient", "gradientFrom": "#ff6ad5", "gradientTo": "#3b82c4", "gradientAngle": 135 } }, "cursor": { "clickEffect": "ripple", "size": 34 } }),
        ),
        (
            "Sunset",
            json!({ "styles": { "paddingRatio": PAD, "cornerRadius": 18, "background": { "kind": "wallpaper", "wallpaper": "sunset" } } }),
        ),
    ]
}

pub fn deep_merge(t: &mut Value, s: &Value) {
    if let (Some(to), Some(so)) = (t.as_object_mut(), s.as_object()) {
        for (k, v) in so {
            if v.is_object() {
                deep_merge(to.entry(k.clone()).or_insert_with(|| json!({})), v);
            } else {
                to.insert(k.clone(), v.clone());
            }
        }
    }
}

pub fn apply_preset_value(p: &mut Project, pr: &Value) {
    // Go through JSON so partial presets merge field by field without overwriting omitted keys.
    for key in ["styles", "cursor", "camera"] {
        let Some(src) = pr.get(key) else { continue };
        match key {
            "styles" => {
                let mut v = serde_json::to_value(&p.config.styles).unwrap_or(Value::Null);
                deep_merge(&mut v, src);
                if let Ok(x) = serde_json::from_value::<Styles>(v) {
                    p.config.styles = x;
                }
            }
            "cursor" => {
                let mut v = serde_json::to_value(&p.config.cursor).unwrap_or(Value::Null);
                deep_merge(&mut v, src);
                if let Ok(x) = serde_json::from_value::<CursorConfig>(v) {
                    p.config.cursor = x;
                }
            }
            _ => {
                let mut v = serde_json::to_value(&p.config.camera).unwrap_or(Value::Null);
                deep_merge(&mut v, src);
                if let Ok(x) = serde_json::from_value::<CameraConfig>(v) {
                    p.config.camera = x;
                }
            }
        }
    }
}

/// Apply one of the built-in subtitle style presets (`classic`, `minimal`, `tiktok`).
pub fn apply_subtitle_preset(name: &str, s: &mut SubtitleStyle) {
    match name {
        "tiktok" => {
            s.style = "tiktok".into();
            s.size = 70.0;
            s.color = "#ffffff".into();
            s.outline_color = "#000000".into();
            s.outline_width = 7.0;
            s.background = String::new();
            s.uppercase = true;
            s.pop = true;
            s.highlight = "#ffe14a".into();
            s.y = 0.8;
            s.width = 0.6;
        }
        "classic" => {
            *s = SubtitleStyle::default();
        }
        _ => {
            s.style = "minimal".into();
            s.size = 42.0;
            s.color = "#ffffff".into();
            s.outline_color = "#000000".into();
            s.outline_width = 3.0;
            s.background = String::new();
            s.uppercase = false;
            s.pop = false;
            s.highlight = String::new();
        }
    }
}

/// Find a preset by name (case-insensitive) in built-ins or `user_presets` and apply it.
pub fn find_and_apply_preset(p: &mut Project, name: &str, user_presets: &[(String, Value)]) -> bool {
    let target = name.trim();
    if let Some((_, val)) = user_presets.iter().find(|(n, _)| n.eq_ignore_ascii_case(target)) {
        apply_preset_value(p, val);
        return true;
    }
    if let Some((_, val)) = builtin_presets().into_iter().find(|(n, _)| n.eq_ignore_ascii_case(target)) {
        apply_preset_value(p, &val);
        return true;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::Media;

    #[test]
    fn merges_presets_and_preserves_custom_padding() {
        let mut p = Project::from_recording("t", Media { duration_ms: 2000.0, ..Default::default() });
        p.config.styles.device = "laptop".into();
        assert!(find_and_apply_preset(&mut p, "clean", &[]));
        assert_eq!(p.config.styles.padding_ratio, 0.02);
        assert_eq!(p.config.styles.corner_radius, 14.0);
        assert_eq!(p.config.styles.device, "laptop");

        let custom = vec![("CustomPad".into(), json!({ "styles": { "paddingRatio": 0.11 } }))];
        assert!(find_and_apply_preset(&mut p, "custompad", &custom));
        assert_eq!(p.config.styles.padding_ratio, 0.11);
        assert_eq!(p.config.styles.corner_radius, 14.0);
    }
}
