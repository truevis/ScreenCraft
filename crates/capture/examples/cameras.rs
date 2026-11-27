//! `cargo run -p screencraft-capture --example cameras [-- --probe]`: list (and optionally probe) cameras.
use screencraft_capture::camera::{list, start, CamPreset};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

fn main() {
    let probe = std::env::args().any(|a| a == "--probe");
    let cams = list();
    if cams.is_empty() {
        println!("No cameras found.");
        return;
    }
    for (id, name) in cams {
        println!("{id}\t{name}");
        if probe {
            let state: Arc<Mutex<(usize, Option<(u32, u32)>)>> = Arc::new(Mutex::new((0, None)));
            let s2 = state.clone();
            match start(Some(&id), CamPreset::Bubble, move |f| {
                let mut g = s2.lock().unwrap();
                g.0 += 1;
                if g.1.is_none() {
                    g.1 = Some((f.width, f.height));
                }
            }) {
                Ok(stream) => {
                    let deadline = Instant::now() + Duration::from_secs(3);
                    while Instant::now() < deadline {
                        if state.lock().unwrap().0 >= 5 {
                            break;
                        }
                        std::thread::sleep(Duration::from_millis(50));
                    }
                    stream.stop();
                    let g = state.lock().unwrap();
                    println!("  probe: {} frames, first={:?}", g.0, g.1);
                }
                Err(e) => println!("  probe error: {e}"),
            }
        }
    }
}
