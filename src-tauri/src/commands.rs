//! Tauri command wrappers. Keep these thin: validation and logic live in `db` and `media`.

use std::path::Path;

use tauri::State;

use crate::AppState;
use crate::db::recipes;
use crate::error::{AppError, AppResult};
use crate::models::{PhotoChange, Recipe, RecipeInput, RecipeSummary};

#[tauri::command]
pub fn new_recipe_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

#[tauri::command]
pub async fn list_recipes(state: State<'_, AppState>) -> AppResult<Vec<RecipeSummary>> {
    recipes::list(&state.db())
}

#[tauri::command]
pub async fn get_recipe(state: State<'_, AppState>, id: String) -> AppResult<Recipe> {
    recipes::get(&state.db(), &id)
}

#[tauri::command]
pub async fn save_recipe(state: State<'_, AppState>, input: RecipeInput) -> AppResult<Recipe> {
    let input = recipes::validate(&input)?;
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
    recipes::upsert(&mut conn, &input, photo.as_deref(), &now)?;
    if let Some(old) = old_photo.filter(|old| Some(old) != photo.as_ref()) {
        state.media.remove(&old);
    }
    recipes::get(&conn, &input.id)
}

/// Deletes the recipe and its photo and video files. The UI confirms first.
#[tauri::command]
pub async fn delete_recipe(state: State<'_, AppState>, id: String) -> AppResult<()> {
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
