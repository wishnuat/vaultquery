pub mod engine;

use engine::{SearchEvent, SearchOpts};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tauri::{ipc::Channel, State};

/// Cancellation flag of the search that is currently running (if any).
#[derive(Default)]
struct AppState {
    current: Mutex<Option<Arc<AtomicBool>>>,
}

#[tauri::command]
async fn search(opts: SearchOpts, on_event: Channel<SearchEvent>, state: State<'_, AppState>) -> Result<(), String> {
    let flag = Arc::new(AtomicBool::new(false));
    if let Some(prev) = state.current.lock().unwrap().replace(flag.clone()) {
        prev.store(true, Ordering::Relaxed); // a new search cancels the previous one
    }
    tauri::async_runtime::spawn_blocking(move || {
        engine::run(opts, flag, &|ev| {
            let _ = on_event.send(ev);
        })
    })
    .await
    .map_err(|e| e.to_string())
}

#[tauri::command]
fn cancel_search(state: State<'_, AppState>) {
    if let Some(flag) = state.current.lock().unwrap().as_ref() {
        flag.store(true, Ordering::Relaxed);
    }
}

#[tauri::command]
fn open_file(path: String) -> Result<(), String> {
    tauri_plugin_opener::open_path(path, None::<&str>).map_err(|e| e.to_string())
}

#[tauri::command]
fn reveal_file(path: String) -> Result<(), String> {
    tauri_plugin_opener::reveal_item_in_dir(path).map_err(|e| e.to_string())
}

#[tauri::command]
fn export_csv(path: String, content: String) -> Result<(), String> {
    // UTF-8 BOM so Excel opens it with the right encoding
    std::fs::write(path, format!("\u{feff}{content}")).map_err(|e| e.to_string())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .manage(AppState::default())
        .invoke_handler(tauri::generate_handler![search, cancel_search, open_file, reveal_file, export_csv])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
