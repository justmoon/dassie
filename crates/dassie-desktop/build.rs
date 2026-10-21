//! Tauri's build step: reads tauri.conf.json and the icons, which
//! `tauri::generate_context!` embeds.

fn main() {
    tauri_build::build();
}
