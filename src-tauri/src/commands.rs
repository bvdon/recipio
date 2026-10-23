//! Tauri command wrappers. Keep these thin: validation and logic live in `db` and `media`.

use std::path::Path;

use tauri::State;

use crate::AppState;
use crate::db::{labels, recipes};
use crate::error::{AppError, AppResult};
use crate::models::{
    Label, LabelKind, PhotoChange, Recipe, RecipeInput, RecipeQuery, RecipeSummary,
};
use crate::quantity;
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
    recipes::get(&state.db(), &id)
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
