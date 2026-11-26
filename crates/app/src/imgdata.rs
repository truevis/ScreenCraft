//! Pick an image and save a downscaled PNG inside the `.screencraft` package folder (keeping
//! `project.json` and the undo stack tiny).

use base64::Engine;
use std::hash::{Hash, Hasher};
use std::path::Path;

pub fn save_image_file(package_dir: &Path, path: &Path, prefix: &str, max_width: u32) -> Result<(String, u32, u32), String> {
    let img = image::open(path).map_err(|e| format!("Could not open that image: {e}"))?;
    let img = if img.width() > max_width {
        let h = (img.height() as f64 * max_width as f64 / img.width() as f64).round().max(1.0) as u32;
        img.resize_exact(max_width, h, image::imageops::FilterType::Lanczos3)
    } else {
        img
    };
    let (w, h) = (img.width().max(1), img.height().max(1));
    let mut bytes = Vec::new();
    img.write_to(&mut std::io::Cursor::new(&mut bytes), image::ImageFormat::Png).map_err(|e| e.to_string())?;
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    bytes.hash(&mut hasher);
    let name = format!("{prefix}-{:016x}.png", hasher.finish());
    let dest = package_dir.join(&name);
    if !dest.exists() {
        std::fs::write(&dest, &bytes).map_err(|e| format!("Could not save image into project: {e}"))?;
    }
    Ok((name, w, h))
}

pub fn pick_and_save_image_with_size(package_dir: &Path, prefix: &str, max_width: u32) -> Result<Option<(String, u32, u32)>, String> {
    let Some(path) = rfd::FileDialog::new().add_filter("Images", &["png", "jpg", "jpeg", "gif", "webp", "bmp", "tiff"]).pick_file() else {
        return Ok(None);
    };
    save_image_file(package_dir, &path, prefix, max_width).map(Some)
}

pub fn pick_and_save_image(package_dir: &Path, prefix: &str, max_width: u32) -> Result<Option<String>, String> {
    pick_and_save_image_with_size(package_dir, prefix, max_width).map(|opt| opt.map(|(name, _, _)| name))
}

/// Decode a `data:image/...;base64,` URL to RGBA pixels.
pub fn decode_data_url(url: &str) -> Option<(u32, u32, Vec<u8>)> {
    let b64 = url.split_once("base64,")?.1;
    let bytes = base64::engine::general_purpose::STANDARD.decode(b64.trim()).ok()?;
    let img = image::load_from_memory(&bytes).ok()?.to_rgba8();
    Some((img.width(), img.height(), img.into_raw()))
}
