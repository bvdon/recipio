mod commands;
mod db;
mod error;
mod media;
mod models;
mod quantity;

use std::collections::HashSet;
use std::sync::{Mutex, MutexGuard};

use rusqlite::Connection;
use tauri::Manager;
use tauri_plugin_log::{RotationStrategy, Target, TargetKind};

use media::MediaDir;

pub struct AppState {
    db: Mutex<Connection>,
    media: MediaDir,
}

impl AppState {
    fn db(&self) -> MutexGuard<'_, Connection> {
        // A panic while holding the lock can't leave SQLite in a torn state
        // (transactions roll back), so recovering from poisoning is safe.
        self.db.lock().unwrap_or_else(|e| e.into_inner())
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .register_asynchronous_uri_scheme_protocol(media::PROTOCOL, |ctx, request, responder| {
            let app = ctx.app_handle().clone();
            tauri::async_runtime::spawn_blocking(move || {
                let response = match app.try_state::<AppState>() {
                    Some(state) => media::handle(&state.media, &request),
                    None => http::Response::builder()
                        .status(http::StatusCode::SERVICE_UNAVAILABLE)
                        .body(Vec::new())
                        .expect("valid response"),
                };
                responder.respond(response);
            });
        })
        .setup(|app| {
            let data_dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&data_dir)?;
            app.handle().plugin(
                tauri_plugin_log::Builder::new()
                    .clear_targets()
                    .target(Target::new(TargetKind::Folder {
                        path: data_dir.join("logs"),
                        file_name: Some("recipio".into()),
                    }))
                    .target(Target::new(TargetKind::Stdout))
                    .rotation_strategy(RotationStrategy::KeepSome(5))
                    .level(log::LevelFilter::Info)
                    .build(),
            )?;
            log::info!("app data dir: {}", data_dir.display());

            let conn = db::open(&data_dir.join("recipio.sqlite3"))?;
            let media = MediaDir::new(&data_dir.join("media"))?;
            let referenced: HashSet<String> =
                db::recipes::all_media_paths(&conn)?.into_iter().collect();
            media.sweep(&referenced);

            app.manage(AppState {
                db: Mutex::new(conn),
                media,
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::new_recipe_id,
            commands::list_recipes,
            commands::get_recipe,
            commands::save_recipe,
            commands::delete_recipe,
            commands::stage_photo,
            commands::list_labels,
            commands::create_label,
            commands::rename_label,
            commands::delete_label,
            commands::merge_labels,
            commands::check_quantity,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
