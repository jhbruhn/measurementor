mod config;
mod oar;
mod ocr;
mod processor;
mod video;

use config::{load_config, save_config};
use processor::{cancel_extract, extract, CancelFlag};
use video::{get_frame, get_video_info};

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(CancelFlag::default())
        .invoke_handler(tauri::generate_handler![
            get_video_info,
            get_frame,
            load_config,
            save_config,
            extract,
            cancel_extract,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
