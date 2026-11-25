//! A small append-only log at `%LOCALAPPDATA%\ScreenCraft\screencraft.log`, so a failed recording
//! or a crash leaves something to read (a GUI app has no console for `eprintln!`).

use std::io::Write;
use std::path::PathBuf;

pub fn path() -> Option<PathBuf> {
    dirs::data_local_dir().map(|d| d.join("ScreenCraft").join("screencraft.log"))
}

pub fn line(msg: &str) {
    eprintln!("[screencraft] {msg}");
    let Some(p) = path() else { return };
    if let Some(dir) = p.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    // keep the file small: start over past 1 MB
    if std::fs::metadata(&p).map(|m| m.len() > 1 << 20).unwrap_or(false) {
        let _ = std::fs::remove_file(&p);
    }
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&p) {
        let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        let _ = writeln!(f, "{secs} {msg}");
    }
}

/// Log panics (thread name, message, location) before the default handler runs.
pub fn install_panic_hook() {
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let thread = std::thread::current().name().unwrap_or("?").to_string();
        line(&format!("panic in thread '{thread}': {info}"));
        prev(info);
    }));
}
