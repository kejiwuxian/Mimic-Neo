//! sai-recorder — Tauri v2 desktop app for opt-in record-and-replay workflow
//! capture with token-efficient compression. MimicCLI-compatible action schema
//! (`types/actions.d.ts`), for the Simular Sai agent and computer-use datasets.
//!
//! The GUI (main window) drives recording; a runtime float window shows the
//! Stop control. The capture/compress/export engine is unchanged from the CLI
//! version and reused as-is.

// Hide the extra console window on Windows release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod actions;
mod capture;
mod commands;
mod compress;
mod export;
mod log;
mod overlay;
mod recorder;
mod review;
mod tasks;
mod telegram;

fn main() {
    // Headless self-test of the capture→compress→export→meta pipeline (no GUI),
    // so the token metrics can be verified without driving the UI.
    if std::env::args().any(|a| a == "--selftest") {
        commands::selftest();
        return;
    }
    // Headless LIVE verification: real worker + injected input + Ctrl+Alt+S hotkey.
    if std::env::args().any(|a| a == "--selftest-record") {
        commands::selftest_record();
        return;
    }

    tauri::Builder::default()
        .manage(commands::AppState::default())
        .invoke_handler(tauri::generate_handler![
            commands::open_float_window,
            commands::start_recording,
            commands::stop_recording,
            commands::recording_state,
            commands::list_tasks,
            commands::get_task,
            commands::rename_task,
            commands::delete_task,
            commands::run_task,
            commands::get_telegram_status,
            commands::send_task_telegram,
        ])
        .run(tauri::generate_context!())
        .expect("error while running sai-recorder");
}
