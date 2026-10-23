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
mod sharing;
mod tools;
mod video;
mod web_import;

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard};

use rusqlite::Connection;
use tauri::{AppHandle, Emitter, Manager};
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
    /// Files waiting for the shared-recipes import screen (see `open_files`).
    opened: Mutex<Vec<String>>,
    outbox: PathBuf,
}

impl AppState {
    fn db(&self) -> MutexGuard<'_, Connection> {
        // A panic while holding the lock can't leave SQLite in a torn state
        // (transactions roll back), so recovering from poisoning is safe.
        self.db.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// Event telling the UI that `.recipio` files were opened (payload: their paths).
const OPEN_FILES_EVENT: &str = "app://open-files";

/// `.recipio` files the user opened (double-click, an attachment in Messages/Mail, or a
/// second launch): queued for the import screen and announced to a running UI.
fn open_files(app: &AppHandle, files: Vec<String>) {
    if files.is_empty() {
        return;
    }
    log::info!("asked to open {files:?}");
    if let Some(state) = app.try_state::<AppState>() {
        state
            .opened
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .extend(files.clone());
    }
    let _ = app.emit(OPEN_FILES_EVENT, files);
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.unminimize();
        let _ = window.set_focus();
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
        // First, so a second launch (e.g. opening a .recipio file on Windows/Linux)
        // hands its files to this instance and exits.
        .plugin(tauri_plugin_single_instance::init(|app, argv, _cwd| {
            open_files(app, sharing::recipio_files(argv.iter().skip(1)));
        }))
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
                opened: Mutex::new(sharing::recipio_files(std::env::args().skip(1))),
                outbox: sharing::outbox(&data_dir),
            });
            sharing::prune_outbox(&sharing::outbox(&data_dir));
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
            commands::preview_shared,
            commands::add_shared_recipes,
            commands::add_video_bookmark,
            commands::delete_video_bookmark,
            commands::export_pdf,
            commands::list_friends,
            commands::add_friend,
            commands::update_friend,
            commands::delete_friend,
            commands::share_history,
            commands::get_my_name,
            commands::set_my_name,
            commands::share,
            commands::reveal_file,
            commands::take_opened_files,
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        // `app` is only needed for the macOS-only `Opened` event.
        .run(
            |#[cfg_attr(not(target_os = "macos"), allow(unused_variables))] app, event| match event
            {
                tauri::RunEvent::Exit => process::kill_all(),
                // macOS delivers opened files (Finder, Messages, Mail attachments) this way.
                #[cfg(target_os = "macos")]
                tauri::RunEvent::Opened { urls } => {
                    open_files(app, sharing::recipio_files(urls.iter().map(|u| u.as_str())));
                }
                _ => {}
            },
        );
}
