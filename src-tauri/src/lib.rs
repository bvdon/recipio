mod backup;
mod commands;
mod db;
mod duration;
mod error;
mod ingredient_parser;
mod jsonld;
mod media;
mod models;
mod pdf;
mod process;
mod quantity;
mod tools;
mod video;
mod web_import;

use std::collections::HashSet;
use std::sync::{Mutex, MutexGuard};

use rusqlite::Connection;
use tauri::Manager;
use tauri_plugin_log::{RotationStrategy, Target, TargetKind};

use backup::Backups;
use media::MediaDir;
use tools::Tools;
use video::Videos;

pub struct AppState {
    db: Mutex<Connection>,
    media: MediaDir,
    tools: Tools,
    videos: Videos,
    backups: Backups,
    /// Held while cooking mode is open (see `commands::set_keep_awake`).
    keep_awake: Mutex<Option<keepawake::KeepAwake>>,
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
    // Release builds abort on panic; record what happened in the log file first.
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        log::error!(
            "panic: {info}\n{}",
            std::backtrace::Backtrace::force_capture()
        );
        default_hook(info);
    }));
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
                // Answer on the main thread. WebKit cancels requests there (the video
                // player does so constantly while seeking); answering from this thread
                // could race a cancellation and abort the app inside wry (crash on
                // 2026-10-23). On the main thread the two can't overlap, and wry skips
                // requests that were already cancelled.
                let main = app.clone();
                if let Err(e) = main.run_on_main_thread(move || responder.respond(response)) {
                    log::warn!("could not answer media request: {e}");
                }
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
            db::recipes::check_videos(&conn, |rel| media.resolve(rel).is_some())?;

            app.manage(AppState {
                db: Mutex::new(conn),
                media,
                tools: Tools::new(&data_dir.join("bin"))?,
                videos: Videos::default(),
                backups: Backups::default(),
                keep_awake: Mutex::new(None),
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
            commands::parse_ingredients,
            commands::check_youtube_url,
            commands::start_video_download,
            commands::cancel_video_download,
            commands::get_video_job,
            commands::dismiss_video_job,
            commands::remove_video,
            commands::tools_status,
            commands::install_tools,
            commands::import_recipe,
            commands::set_keep_awake,
            commands::library_stats,
            commands::export_backup,
            commands::cancel_backup,
            commands::inspect_backup,
            commands::restore_backup,
            commands::share_recipes,
            commands::preview_shared,
            commands::add_shared_recipes,
            commands::add_video_bookmark,
            commands::delete_video_bookmark,
            commands::export_pdf,
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|_app, event| {
            if let tauri::RunEvent::Exit = event {
                process::kill_all();
            }
        });
}
