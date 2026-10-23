//! Tauri command wrappers. Keep these thin: validation and logic live in `db` and `media`.

use std::path::{Path, PathBuf};

use tauri::{AppHandle, Emitter, Manager, State};

use crate::AppState;
use crate::backup::{self, BackupSummary, ExportOptions, Kind, LibraryStats, SharePreview};
use crate::db::{bookmarks, friends, labels, recipes};
use crate::error::{AppError, AppResult};
use crate::models::{
    Friend, FriendInput, ShareDirection, ShareLogEntry, VideoBookmark, VideoStatus,
};
use crate::models::{
    Label, LabelKind, PhotoChange, Recipe, RecipeInput, RecipeQuery, RecipeSummary, WebImport,
};
use crate::quantity;
use crate::sharing::{self, ShareFormat, ShareOutcome, ShareTarget};
use crate::tools::{Package, ToolInfo};
use crate::video::{self, VideoJob};
use crate::web_import;
use crate::{ingredient_parser, models::ParsedIngredient};

#[tauri::command]
pub fn new_recipe_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

#[tauri::command]
pub async fn list_recipes(
    state: State<'_, AppState>,
    query: RecipeQuery,
) -> AppResult<Vec<RecipeSummary>> {
    recipes::list(&state.db(), &query)
}

#[tauri::command]
pub async fn get_recipe(state: State<'_, AppState>, id: String) -> AppResult<Recipe> {
    let conn = state.db();
    let mut recipe = recipes::get(&conn, &id)?;
    // The file may have been removed while the app was running.
    if recipe.video_status == VideoStatus::Ready
        && !recipe
            .video_path
            .as_deref()
            .is_some_and(|p| state.media.resolve(p).is_some())
    {
        recipes::set_video(&conn, &id, None, VideoStatus::Missing, None)?;
        recipe.video_path = None;
        recipe.video_status = VideoStatus::Missing;
    }
    Ok(recipe)
}

#[tauri::command]
pub async fn save_recipe(state: State<'_, AppState>, input: RecipeInput) -> AppResult<Recipe> {
    let recipe = recipes::validate(&input)?;
    let input = &recipe.input;
    let mut conn = state.db();
    let old_photo = recipes::media_of(&conn, &input.id)?.and_then(|m| m.photo_path);
    let photo = match &input.photo {
        PhotoChange::Keep => old_photo.clone(),
        PhotoChange::Remove => None,
        PhotoChange::Replace { staged_path } => {
            Some(state.media.commit_photo(&input.id, staged_path)?)
        }
    };
    let now = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    recipes::upsert(&mut conn, &recipe, photo.as_deref(), &now)?;
    // A video that finished downloading before the recipe's first save.
    let video = format!("videos/{}.mp4", input.id);
    let linked = recipes::media_of(&conn, &input.id)?.and_then(|m| m.video_path);
    if linked.is_none()
        && !state.videos.is_running(&input.id)
        && state.media.resolve(&video).is_some()
    {
        recipes::set_video(&conn, &input.id, Some(&video), VideoStatus::Ready, None)?;
    }
    if let Some(old) = old_photo.filter(|old| Some(old) != photo.as_ref()) {
        state.media.remove(&old);
    }
    recipes::get(&conn, &input.id)
}

/// Deletes the recipe and its photo and video files. The UI confirms first.
#[tauri::command]
pub async fn delete_recipe(state: State<'_, AppState>, id: String) -> AppResult<()> {
    state.videos.cancel(&id);
    let media = recipes::delete(&mut state.db(), &id)?.ok_or(AppError::NotFound("Recipe"))?;
    for rel in [media.photo_path, media.video_path].into_iter().flatten() {
        state.media.remove(&rel);
    }
    Ok(())
}

/// Copies an image chosen in the file dialog into the staging area.
#[tauri::command]
pub async fn stage_photo(state: State<'_, AppState>, path: String) -> AppResult<String> {
    state.media.stage_photo(Path::new(&path))
}

#[tauri::command]
pub async fn list_labels(state: State<'_, AppState>, kind: LabelKind) -> AppResult<Vec<Label>> {
    labels::list(&state.db(), kind)
}

#[tauri::command]
pub async fn create_label(
    state: State<'_, AppState>,
    kind: LabelKind,
    name: String,
) -> AppResult<i64> {
    labels::create(&state.db(), kind, &name)
}

#[tauri::command]
pub async fn rename_label(
    state: State<'_, AppState>,
    kind: LabelKind,
    id: i64,
    name: String,
) -> AppResult<()> {
    labels::rename(&state.db(), kind, id, &name)
}

/// Removes the label from all recipes; the recipes are kept. The UI confirms first.
#[tauri::command]
pub async fn delete_label(state: State<'_, AppState>, kind: LabelKind, id: i64) -> AppResult<()> {
    labels::delete(&state.db(), kind, id)
}

#[tauri::command]
pub async fn merge_labels(
    state: State<'_, AppState>,
    kind: LabelKind,
    sources: Vec<i64>,
    target: i64,
) -> AppResult<()> {
    labels::merge(&mut state.db(), kind, &sources, target)
}

/// Whether quantity text as typed in the editor can be parsed (live validation; a plain
/// bool so invalid keystrokes don't log errors).
#[tauri::command]
pub fn check_quantity(text: String) -> bool {
    quantity::parse(&text).is_ok()
}

/// Splits pasted ingredient text into lines for review (paste-to-parse). Never fails:
/// anything not understood lands in `item`.
#[tauri::command]
pub fn parse_ingredients(text: String) -> Vec<ParsedIngredient> {
    ingredient_parser::parse_block(&text)
}

/// Whether text is a YouTube video link Recipio can download (live check in the editor).
#[tauri::command]
pub fn check_youtube_url(url: String) -> bool {
    video::youtube_url(&url).is_some()
}

/// Starts a background download; progress arrives as `video://job` events.
#[tauri::command]
pub fn start_video_download(app: AppHandle, recipe_id: String, url: String) -> AppResult<VideoJob> {
    uuid::Uuid::parse_str(&recipe_id)
        .map_err(|_| AppError::Validation("Invalid recipe id.".into()))?;
    video::start(&app, &recipe_id, &url)
}

#[tauri::command]
pub fn cancel_video_download(state: State<'_, AppState>, recipe_id: String) {
    state.videos.cancel(&recipe_id);
}

/// The recipe's current or last download, so a page opened mid-download can show it.
#[tauri::command]
pub fn get_video_job(state: State<'_, AppState>, recipe_id: String) -> Option<VideoJob> {
    state.videos.get(&recipe_id)
}

#[tauri::command]
pub fn dismiss_video_job(state: State<'_, AppState>, recipe_id: String) {
    state.videos.dismiss(&recipe_id);
}

/// Deletes the recipe's video file right away (the UI confirms first). The YouTube URL
/// is kept so it can be downloaded again.
#[tauri::command]
pub async fn remove_video(state: State<'_, AppState>, recipe_id: String) -> AppResult<()> {
    state.videos.cancel(&recipe_id);
    state.videos.dismiss(&recipe_id);
    let conn = state.db();
    if let Some(path) = recipes::media_of(&conn, &recipe_id)?.and_then(|m| m.video_path) {
        state.media.remove(&path);
    }
    // Also a finished download of a recipe that hasn't been saved yet.
    state.media.remove(&format!("videos/{recipe_id}.mp4"));
    recipes::set_video(&conn, &recipe_id, None, VideoStatus::None, None)?;
    Ok(())
}

#[tauri::command]
pub fn tools_status(state: State<'_, AppState>) -> Vec<ToolInfo> {
    state.tools.status()
}

/// Installs missing tools; with `update`, also replaces outdated ones. Progress arrives
/// as `tools://progress` events. Returns the packages that changed.
#[tauri::command]
pub async fn install_tools(
    app: AppHandle,
    state: State<'_, AppState>,
    update: bool,
) -> AppResult<Vec<Package>> {
    let progress = |p| {
        let _ = app.emit("tools://progress", p);
    };
    state.tools.install(update, &progress).await
}

/// Reads a recipe from a web page for the editor to show. Nothing is saved; a staged
/// photo that is never saved is removed on the next start.
#[tauri::command]
pub async fn import_recipe(state: State<'_, AppState>, url: String) -> AppResult<WebImport> {
    web_import::import(&state, &url).await
}

/// Keeps the screen on while cooking mode is open. Not `async` on purpose: sync commands
/// run on the main thread, and on Windows the keep-awake request belongs to the thread
/// that made it. Failure only means the screen may dim, so it's logged, not raised.
#[tauri::command]
pub fn set_keep_awake(state: State<'_, AppState>, on: bool) {
    let mut held = state.keep_awake.lock().unwrap_or_else(|e| e.into_inner());
    if !on {
        *held = None;
    } else if held.is_none() {
        match keepawake::Builder::default()
            .display(true)
            .idle(true)
            .reason("Cooking mode is open")
            .app_name("Recipio")
            .app_reverse_domain("io.github.bvdon.recipio")
            .create()
        {
            Ok(awake) => *held = Some(awake),
            Err(e) => log::warn!("could not keep the screen awake: {e}"),
        }
    }
}

/// Contents of the whole library, or of the recipes in `ids` (sharing).
#[tauri::command]
pub async fn library_stats(
    state: State<'_, AppState>,
    ids: Option<Vec<String>>,
) -> AppResult<LibraryStats> {
    backup::stats(&state, ids.as_deref())
}

/// Writes the whole library to a zip at `path` (chosen in a save dialog). Progress
/// arrives as `backup://progress` events.
#[tauri::command]
pub async fn export_backup(app: AppHandle, path: String, include_videos: bool) -> AppResult<()> {
    blocking(app, move |app, state| {
        backup::export(
            state,
            Path::new(&path),
            &ExportOptions::backup(include_videos),
            &|p| {
                let _ = app.emit(backup::EVENT, p);
            },
        )
    })
    .await
}

#[tauri::command]
pub fn cancel_backup(state: State<'_, AppState>) {
    state.backups.cancel();
}

/// Reads a backup's summary so the UI can confirm before restoring.
#[tauri::command]
pub async fn inspect_backup(path: String) -> AppResult<BackupSummary> {
    let path = PathBuf::from(path);
    tauri::async_runtime::spawn_blocking(move || backup::inspect(&path))
        .await
        .map_err(|e| AppError::Io(std::io::Error::other(e)))?
}

/// Replaces the whole library with a backup (the UI warns first). Returns the number of
/// recipes restored.
#[tauri::command]
pub async fn restore_backup(app: AppHandle, path: String) -> AppResult<u32> {
    blocking(app, move |app, state| {
        backup::restore(state, Path::new(&path), &|p| {
            let _ = app.emit(backup::EVENT, p);
        })
    })
    .await
}

/// Lists the recipes in a shared file so the user can pick which to add.
#[tauri::command]
pub async fn preview_shared(app: AppHandle, path: String) -> AppResult<SharePreview> {
    blocking(app, move |_, state| {
        backup::preview(state, Path::new(&path))
    })
    .await
}

/// Adds the chosen recipes from a shared file; never changes existing recipes.
#[tauri::command]
pub async fn add_shared_recipes(app: AppHandle, path: String, ids: Vec<String>) -> AppResult<u32> {
    blocking(app, move |app, state| {
        backup::add_shared(state, Path::new(&path), &ids, &|p| {
            let _ = app.emit(backup::EVENT, p);
        })
    })
    .await
}

/// Adds a bookmark at `time` as typed ("2:15", "90", "1m30s"). `duration` is the video's
/// length from the player, to reject times past the end. Returns the updated list.
#[tauri::command]
pub async fn add_video_bookmark(
    state: State<'_, AppState>,
    recipe_id: String,
    time: String,
    label: String,
    duration: Option<f64>,
) -> AppResult<Vec<VideoBookmark>> {
    let seconds = crate::duration::timestamp(&time).ok_or_else(|| {
        AppError::Validation("Enter a time in the video, like 2:15 or 1:02:30.".into())
    })?;
    let conn = state.db();
    bookmarks::add(&conn, &recipe_id, seconds, &label, duration)?;
    bookmarks::list(&conn, &recipe_id)
}

#[tauri::command]
pub async fn delete_video_bookmark(
    state: State<'_, AppState>,
    recipe_id: String,
    id: i64,
) -> AppResult<Vec<VideoBookmark>> {
    let conn = state.db();
    bookmarks::delete(&conn, &recipe_id, id)?;
    bookmarks::list(&conn, &recipe_id)
}

/// Saves the recipe as a PDF at `path` (chosen in a save dialog) and opens it. Text and
/// photo only, never the video. `locale` (the UI language, e.g. "en-US") picks Letter or A4.
#[tauri::command]
pub async fn export_pdf(app: AppHandle, id: String, path: String, locale: String) -> AppResult<()> {
    blocking(app, move |app, state| {
        let recipe = recipes::get(&state.db(), &id)?;
        let photo = pdf_photo(state, &recipe);
        let bytes = crate::pdf::render(&recipe, photo, &locale)?;
        std::fs::write(&path, bytes)?;
        log::info!("saved PDF of {id} to {path}");
        use tauri_plugin_opener::OpenerExt;
        if let Err(e) = app.opener().open_path(&path, None::<&str>) {
            log::warn!("could not open the PDF: {e}");
        }
        Ok(())
    })
    .await
}

fn pdf_photo(state: &AppState, recipe: &crate::models::Recipe) -> Option<crate::pdf::Photo> {
    let file = state.media.resolve(recipe.photo_path.as_deref()?)?;
    Some(crate::pdf::Photo {
        ext: file.extension()?.to_str()?.to_ascii_lowercase(),
        data: std::fs::read(&file).ok()?,
    })
}

#[tauri::command]
pub async fn list_friends(state: State<'_, AppState>) -> AppResult<Vec<Friend>> {
    friends::list(&state.db())
}

#[tauri::command]
pub async fn add_friend(state: State<'_, AppState>, input: FriendInput) -> AppResult<Friend> {
    let now = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    friends::add(&state.db(), &input, &now)
}

#[tauri::command]
pub async fn update_friend(
    state: State<'_, AppState>,
    id: i64,
    input: FriendInput,
) -> AppResult<Friend> {
    friends::update(&state.db(), id, &input)
}

/// Removes a friend (the UI confirms first); the history of what was shared stays.
#[tauri::command]
pub async fn delete_friend(state: State<'_, AppState>, id: i64) -> AppResult<()> {
    friends::delete(&state.db(), id)
}

/// What was sent and received, newest first (one friend's, or everyone's).
#[tauri::command]
pub async fn share_history(
    state: State<'_, AppState>,
    friend_id: Option<i64>,
) -> AppResult<Vec<ShareLogEntry>> {
    friends::history(&state.db(), friend_id)
}

/// The user's name as shown to friends in shared files ("" until set).
#[tauri::command]
pub async fn get_my_name(state: State<'_, AppState>) -> AppResult<String> {
    friends::my_name(&state.db())
}

#[tauri::command]
pub async fn set_my_name(state: State<'_, AppState>, name: String) -> AppResult<String> {
    friends::set_my_name(&state.db(), &name)
}

/// What to share, as what, and where to.
#[derive(Debug, serde::Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct ShareRequest {
    pub ids: Vec<String>,
    pub format: ShareFormat,
    /// Recipio files only.
    pub include_videos: bool,
    /// Goes with the file (message text, and inside a Recipio file).
    pub note: String,
    pub target: ShareTarget,
    /// The UI language, for the PDF's paper size.
    pub locale: String,
}

/// Writes the chosen recipes as a Recipio file or PDF and hands it to a friend's app,
/// the share menu, or a file the user picked. Progress arrives as `backup://progress`.
#[tauri::command]
pub async fn share(app: AppHandle, request: ShareRequest) -> AppResult<ShareOutcome> {
    blocking(app, move |app, state| {
        let r = request;
        if r.ids.is_empty() {
            return Err(AppError::Validation(
                "Choose at least one recipe to share.".into(),
            ));
        }
        let (from, list, friend) = {
            let conn = state.db();
            let list = r
                .ids
                .iter()
                .map(|id| recipes::get(&conn, id))
                .collect::<AppResult<Vec<_>>>()?;
            let friend = match r.target {
                ShareTarget::Friend { id } => Some(friends::get(&conn, id)?),
                _ => None,
            };
            (friends::my_name(&conn)?, list, friend)
        };
        let titles: Vec<String> = list.iter().map(|x| x.title.clone()).collect();
        let name = sharing::file_name(&titles, &from, r.format);
        let path = match &r.target {
            ShareTarget::File { path } => PathBuf::from(path),
            _ => {
                std::fs::create_dir_all(&state.outbox)?;
                sharing::unique_path(&state.outbox, &name)
            }
        };
        match r.format {
            ShareFormat::Recipio => backup::export(
                state,
                &path,
                &ExportOptions {
                    kind: Kind::Share,
                    ids: Some(&r.ids),
                    include_videos: r.include_videos,
                    from: from.clone(),
                    note: r.note.clone(),
                },
                &|p| {
                    let _ = app.emit(backup::EVENT, p);
                },
            )?,
            ShareFormat::Pdf => {
                let doc_title = name.trim_end_matches(".pdf").to_string();
                let items = list.into_iter().map(|x| {
                    let photo = pdf_photo(state, &x);
                    (x, photo)
                });
                let bytes = crate::pdf::render_many(items.collect(), &doc_title, &r.locale)?;
                std::fs::write(&path, bytes)?;
            }
        }
        let bytes = std::fs::metadata(&path)?.len();
        let subject = sharing::subject(&titles, &from);
        let text = sharing::message(&r.note, r.format, titles.len());
        let handoff = match (&r.target, friend) {
            (ShareTarget::Friend { .. }, Some(f)) => {
                sharing::send_to_friend(app, &path, &f, &subject, &text)
            }
            (ShareTarget::Other { anchor }, _) => sharing::send_other(app, &path, &text, *anchor),
            _ => sharing::Handoff::Saved,
        };
        if !matches!(r.target, ShareTarget::File { .. }) {
            let now = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
            let (friend_id, person) = match (&r.target, friends::list(&state.db())?) {
                (ShareTarget::Friend { id }, all) => (
                    Some(*id),
                    all.into_iter()
                        .find(|f| f.id == *id)
                        .map(|f| f.name)
                        .unwrap_or_default(),
                ),
                _ => (None, String::new()),
            };
            let file = path.file_name().unwrap_or_default().to_string_lossy();
            friends::log(
                &state.db(),
                friend_id,
                ShareDirection::Sent,
                &person,
                &titles,
                &file,
                &now,
            )?;
        }
        log::info!(
            "shared {} recipes as {} ({handoff:?})",
            titles.len(),
            path.display()
        );
        Ok(ShareOutcome {
            file: path.to_string_lossy().into_owned(),
            bytes,
            handoff,
        })
    })
    .await
}

/// Shows a file in Finder / Explorer / the file manager.
#[tauri::command]
pub fn reveal_file(app: AppHandle, path: String) {
    sharing::reveal(&app, Path::new(&path));
}

/// `.recipio` files opened before the UI was ready (or since it last asked).
#[tauri::command]
pub fn take_opened_files(state: State<'_, AppState>) -> Vec<String> {
    std::mem::take(&mut *state.opened.lock().unwrap_or_else(|e| e.into_inner()))
}

/// Runs file-heavy work off the async runtime.
async fn blocking<T: Send + 'static>(
    app: AppHandle,
    work: impl FnOnce(&AppHandle, &AppState) -> AppResult<T> + Send + 'static,
) -> AppResult<T> {
    tauri::async_runtime::spawn_blocking(move || work(&app, &app.state::<AppState>()))
        .await
        .map_err(|e| AppError::Io(std::io::Error::other(e)))?
}

#[cfg(test)]
mod tests {
    /// Holds a keep-awake for a few seconds: `cargo test -- --ignored live_keep_awake`
    /// (check with `pmset -g assertions` on macOS meanwhile).
    #[test]
    #[ignore]
    fn live_keep_awake() {
        let _awake = keepawake::Builder::default()
            .display(true)
            .idle(true)
            .reason("Cooking mode is open")
            .create()
            .unwrap();
        std::thread::sleep(std::time::Duration::from_secs(8));
    }
}
