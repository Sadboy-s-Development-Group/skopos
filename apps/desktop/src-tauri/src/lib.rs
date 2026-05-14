//! Skopos desktop UI shell.
//!
//! This crate is intentionally thin: it owns the Tauri window/runtime and
//! exposes `#[tauri::command]`s that bridge the React frontend to the Skopos
//! workspace crates (`skopos-store`, `skopos-collectors`, ...). Wire those in
//! as the core MVP stabilizes.

/// Minimal health-check command so the frontend can confirm the Rust backend
/// is reachable. Replace/extend with real usage queries.
#[tauri::command]
fn app_status() -> String {
    "bootstrapped".to_string()
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![app_status])
        .run(tauri::generate_context!())
        .expect("error while running Skopos desktop application");
}
