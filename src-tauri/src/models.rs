//! Types shared with the frontend. `cargo test` regenerates `src/bindings/*.ts`.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum VideoStatus {
    None,
    Downloading,
    Ready,
    Failed,
    Missing,
}

impl VideoStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            VideoStatus::None => "none",
            VideoStatus::Downloading => "downloading",
            VideoStatus::Ready => "ready",
            VideoStatus::Failed => "failed",
            VideoStatus::Missing => "missing",
        }
    }

    pub fn parse(s: &str) -> Self {
        match s {
            "downloading" => VideoStatus::Downloading,
            "ready" => VideoStatus::Ready,
            "failed" => VideoStatus::Failed,
            "missing" => VideoStatus::Missing,
            _ => VideoStatus::None,
        }
    }
}

/// One ingredient line, or a section heading when `is_heading` is true
/// (the heading text is then in `item` and the other fields are empty).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct IngredientLine {
    pub is_heading: bool,
    pub quantity: Option<f64>,
    /// Upper bound for ranges such as "2-3".
    pub quantity_max: Option<f64>,
    pub unit: Option<String>,
    pub item: String,
    pub prep_note: Option<String>,
    /// `quantity`/`quantity_max` formatted for display, e.g. "1½" or "2–3"; empty if none.
    pub quantity_text: String,
}

/// An ingredient line as entered in the editor; `quantity` is the raw text
/// ("1 1/2", "¾", "2-3"), parsed by Rust on save.
#[derive(Debug, Clone, PartialEq, Deserialize, TS)]
#[ts(export)]
pub struct IngredientInput {
    pub is_heading: bool,
    pub quantity: String,
    pub unit: Option<String>,
    pub item: String,
    pub prep_note: Option<String>,
}

/// An ingredient line picked apart by paste-to-parse, shaped like an editor row so the
/// UI can show it for review and then drop it into the editor as-is. `quantity` is
/// display text the quantity parser accepts back ("1½", "2–3"); unknown parts are "".
#[derive(Debug, Clone, Default, PartialEq, Serialize, TS)]
#[ts(export)]
pub struct ParsedIngredient {
    pub is_heading: bool,
    pub quantity: String,
    pub unit: String,
    pub item: String,
    pub prep_note: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Nutrition {
    pub calories: Option<f64>,
    pub protein_g: Option<f64>,
    pub carbs_g: Option<f64>,
    pub fat_g: Option<f64>,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct Recipe {
    pub id: String,
    pub title: String,
    pub description: String,
    pub servings: Option<u32>,
    pub prep_minutes: Option<u32>,
    pub cook_minutes: Option<u32>,
    /// Path relative to the media folder, e.g. `photos/<id>.jpg`.
    pub photo_path: Option<String>,
    pub notes: String,
    pub rating: Option<u8>,
    pub youtube_url: Option<String>,
    pub video_path: Option<String>,
    pub video_status: VideoStatus,
    pub source_url: Option<String>,
    pub nutrition: Nutrition,
    pub ingredients: Vec<IngredientLine>,
    pub steps: Vec<String>,
    pub categories: Vec<String>,
    pub tags: Vec<String>,
    pub created_at: String,
    pub updated_at: String,
}

/// Lightweight row for the library grid/list.
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct RecipeSummary {
    pub id: String,
    pub title: String,
    pub photo_path: Option<String>,
    pub rating: Option<u8>,
    pub has_video: bool,
    pub categories: Vec<String>,
    pub created_at: String,
    pub updated_at: String,
}

/// What happens to the photo when a recipe is saved.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(tag = "action", rename_all = "snake_case")]
#[ts(export)]
pub enum PhotoChange {
    Keep,
    Remove,
    /// Use a file previously staged with `stage_photo` (path relative to the media folder).
    Replace {
        staged_path: String,
    },
}

/// Payload for creating or updating a recipe. Upserts by `id`.
#[derive(Debug, Clone, Deserialize, TS)]
#[ts(export)]
pub struct RecipeInput {
    pub id: String,
    pub title: String,
    pub description: String,
    pub servings: Option<u32>,
    pub prep_minutes: Option<u32>,
    pub cook_minutes: Option<u32>,
    pub photo: PhotoChange,
    pub notes: String,
    pub rating: Option<u8>,
    pub youtube_url: Option<String>,
    pub source_url: Option<String>,
    pub nutrition: Nutrition,
    pub ingredients: Vec<IngredientInput>,
    pub steps: Vec<String>,
    /// Category and tag names; unknown names are created on save.
    pub categories: Vec<String>,
    pub tags: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum LabelKind {
    Category,
    Tag,
}

/// A category or tag, with the number of recipes using it.
#[derive(Debug, Clone, PartialEq, Serialize, TS)]
#[ts(export)]
pub struct Label {
    #[ts(type = "number")]
    pub id: i64,
    pub name: String,
    pub recipe_count: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum RecipeSort {
    Title,
    Created,
    Updated,
    Rating,
}

/// Library search, filters and sort. All filters combine with AND.
#[derive(Debug, Clone, Deserialize, TS)]
#[ts(export)]
pub struct RecipeQuery {
    /// Free text, matched against the full-text index (prefix match per word).
    pub search: String,
    #[ts(type = "number[]")]
    pub category_ids: Vec<i64>,
    #[ts(type = "number[]")]
    pub tag_ids: Vec<i64>,
    pub sort: RecipeSort,
    /// Recipes without a rating always sort last.
    pub descending: bool,
}
