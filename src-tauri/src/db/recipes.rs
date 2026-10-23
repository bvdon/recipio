use rusqlite::types::Value;
use rusqlite::{Connection, OptionalExtension, Row, Transaction, params, params_from_iter};

use super::labels;
use crate::error::{AppError, AppResult};
use crate::models::{
    IngredientInput, IngredientLine, LabelKind, Nutrition, Recipe, RecipeInput, RecipeQuery,
    RecipeSort, RecipeSummary, VideoStatus,
};
use crate::quantity;

/// Media files owned by a recipe, relative to the media folder.
#[derive(Debug, Default, PartialEq)]
pub struct RecipeMedia {
    pub photo_path: Option<String>,
    pub video_path: Option<String>,
}

/// Library listing: full-text search, category and tag filters (all ANDed), sorted.
pub fn list(conn: &Connection, query: &RecipeQuery) -> AppResult<Vec<RecipeSummary>> {
    let mut sql = String::from(
        "SELECT id, title, photo_path, rating, video_status, created_at, updated_at
         FROM recipes r WHERE 1",
    );
    let mut args: Vec<Value> = Vec::new();
    if let Some(fts) = fts_query(&query.search) {
        sql.push_str(" AND r.id IN (SELECT recipe_id FROM recipes_fts WHERE recipes_fts MATCH ?)");
        args.push(fts.into());
    }
    for &id in &query.category_ids {
        sql.push_str(
            " AND EXISTS (SELECT 1 FROM recipe_categories
                          WHERE recipe_id = r.id AND category_id = ?)",
        );
        args.push(id.into());
    }
    for &id in &query.tag_ids {
        sql.push_str(
            " AND EXISTS (SELECT 1 FROM recipe_tags WHERE recipe_id = r.id AND tag_id = ?)",
        );
        args.push(id.into());
    }
    let dir = if query.descending { "DESC" } else { "ASC" };
    let order = match query.sort {
        RecipeSort::Title => format!("r.title COLLATE NOCASE {dir}"),
        RecipeSort::Created => format!("r.created_at {dir}"),
        RecipeSort::Updated => format!("r.updated_at {dir}"),
        RecipeSort::Rating => format!("r.rating IS NULL, r.rating {dir}"),
    };
    // Title, then id, as tie-breakers so the order is stable.
    sql.push_str(&format!(" ORDER BY {order}, r.title COLLATE NOCASE, r.id"));

    let mut stmt = conn.prepare(&sql)?;
    let mut summaries = stmt
        .query_map(params_from_iter(args), |row| {
            Ok(RecipeSummary {
                id: row.get(0)?,
                title: row.get(1)?,
                photo_path: row.get(2)?,
                rating: row.get(3)?,
                has_video: VideoStatus::parse(&row.get::<_, String>(4)?) == VideoStatus::Ready,
                categories: Vec::new(),
                created_at: row.get(5)?,
                updated_at: row.get(6)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    for s in &mut summaries {
        s.categories = categories_of(conn, &s.id)?;
    }
    Ok(summaries)
}

/// Turns free text into a safe FTS5 query: each word becomes a quoted prefix term, and
/// all terms must match. Returns `None` when there is nothing searchable.
pub fn fts_query(search: &str) -> Option<String> {
    let terms: Vec<String> = search
        .split_whitespace()
        .map(|w| w.replace('"', ""))
        .filter(|w| w.chars().any(char::is_alphanumeric))
        .map(|w| format!("\"{w}\"*"))
        .collect();
    (!terms.is_empty()).then(|| terms.join(" "))
}

pub fn get(conn: &Connection, id: &str) -> AppResult<Recipe> {
    let mut recipe = conn
        .query_row(
            "SELECT id, title, description, servings, prep_minutes, cook_minutes, photo_path,
                    notes, rating, youtube_url, video_path, video_status, source_url,
                    calories, protein_g, carbs_g, fat_g, created_at, updated_at
             FROM recipes WHERE id = ?1",
            [id],
            recipe_from_row,
        )
        .optional()?
        .ok_or(AppError::NotFound("Recipe"))?;
    recipe.ingredients = ingredients_of(conn, id)?;
    recipe.steps = steps_of(conn, id)?;
    recipe.categories = categories_of(conn, id)?;
    recipe.tags = tags_of(conn, id)?;
    Ok(recipe)
}

pub fn media_of(conn: &Connection, id: &str) -> AppResult<Option<RecipeMedia>> {
    Ok(conn
        .query_row(
            "SELECT photo_path, video_path FROM recipes WHERE id = ?1",
            [id],
            |row| {
                Ok(RecipeMedia {
                    photo_path: row.get(0)?,
                    video_path: row.get(1)?,
                })
            },
        )
        .optional()?)
}

/// Records a recipe's video file and status. With `source`, also stores the YouTube URL
/// it came from and bumps `updated_at` (a finished download). Returns false if the recipe
/// doesn't exist (not saved yet).
pub fn set_video(
    conn: &Connection,
    id: &str,
    path: Option<&str>,
    status: VideoStatus,
    source: Option<(&str, &str)>,
) -> AppResult<bool> {
    let n = match source {
        Some((url, now)) => conn.execute(
            "UPDATE recipes SET video_path = ?2, video_status = ?3, youtube_url = ?4,
                                updated_at = ?5
             WHERE id = ?1",
            params![id, path, status.as_str(), url, now],
        )?,
        None => conn.execute(
            "UPDATE recipes SET video_path = ?2, video_status = ?3 WHERE id = ?1",
            params![id, path, status.as_str()],
        )?,
    };
    Ok(n > 0)
}

/// Startup check: downloads can't survive a restart, so `downloading` becomes `failed`
/// (retry is offered), and a `ready` video whose file is gone becomes `missing`
/// (re-download is offered). `exists` tells whether a media-relative file is present.
pub fn check_videos(conn: &Connection, exists: impl Fn(&str) -> bool) -> AppResult<()> {
    conn.execute(
        "UPDATE recipes SET video_status = 'failed' WHERE video_status = 'downloading'",
        [],
    )?;
    let ready: Vec<(String, Option<String>)> = conn
        .prepare("SELECT id, video_path FROM recipes WHERE video_status = 'ready'")?
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<Result<_, _>>()?;
    for (id, path) in ready {
        if !path.as_deref().is_some_and(&exists) {
            log::warn!("video for {id} is missing ({path:?})");
            set_video(conn, &id, None, VideoStatus::Missing, None)?;
        }
    }
    Ok(())
}

/// A recipe that passed validation: cleaned input plus parsed ingredient lines.
#[derive(Debug, Clone)]
pub struct ValidRecipe {
    pub input: RecipeInput,
    pub ingredients: Vec<IngredientLine>,
}

/// Checks the input and returns a cleaned copy (trimmed text, blank lines removed,
/// quantities parsed).
pub fn validate(input: &RecipeInput) -> AppResult<ValidRecipe> {
    let mut input = input.clone();
    if uuid::Uuid::parse_str(&input.id).is_err() {
        return Err(AppError::Validation("Invalid recipe id.".into()));
    }
    input.title = input.title.trim().to_string();
    if input.title.is_empty() {
        return Err(AppError::Validation(
            "Please give the recipe a title.".into(),
        ));
    }
    if matches!(input.rating, Some(r) if !(1..=5).contains(&r)) {
        return Err(AppError::Validation(
            "Rating must be between 1 and 5.".into(),
        ));
    }
    if input.servings == Some(0) {
        return Err(AppError::Validation("Servings must be at least 1.".into()));
    }
    let ingredients = input
        .ingredients
        .iter()
        .map(clean_line)
        .collect::<AppResult<Vec<_>>>()?
        .into_iter()
        .filter(|l| !l.item.is_empty())
        .collect();
    input.steps = input
        .steps
        .into_iter()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    input.youtube_url = non_blank(input.youtube_url);
    input.source_url = non_blank(input.source_url);
    input.categories = labels::clean_names(LabelKind::Category, &input.categories)?;
    input.tags = labels::clean_names(LabelKind::Tag, &input.tags)?;
    Ok(ValidRecipe { input, ingredients })
}

/// Inserts or updates a recipe (by id) together with its ingredients, steps, categories,
/// tags and search index entry. `input` must already be validated. Video fields are left untouched.
pub fn upsert(
    conn: &mut Connection,
    recipe: &ValidRecipe,
    photo_path: Option<&str>,
    now: &str,
) -> AppResult<()> {
    let tx = conn.transaction()?;
    write(&tx, recipe, photo_path, now)?;
    tx.commit()?;
    Ok(())
}

/// The body of `upsert`, inside a caller's transaction (backup restore writes many).
pub fn write(
    tx: &Transaction,
    recipe: &ValidRecipe,
    photo_path: Option<&str>,
    now: &str,
) -> AppResult<()> {
    let input = &recipe.input;
    let n = &input.nutrition;
    tx.execute(
        "INSERT INTO recipes (id, title, description, servings, prep_minutes, cook_minutes,
                              photo_path, notes, rating, youtube_url, source_url,
                              calories, protein_g, carbs_g, fat_g, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?16)
         ON CONFLICT(id) DO UPDATE SET
            title = excluded.title, description = excluded.description,
            servings = excluded.servings, prep_minutes = excluded.prep_minutes,
            cook_minutes = excluded.cook_minutes, photo_path = excluded.photo_path,
            notes = excluded.notes, rating = excluded.rating,
            youtube_url = excluded.youtube_url, source_url = excluded.source_url,
            calories = excluded.calories, protein_g = excluded.protein_g,
            carbs_g = excluded.carbs_g, fat_g = excluded.fat_g,
            updated_at = excluded.updated_at",
        params![
            input.id,
            input.title,
            input.description,
            input.servings,
            input.prep_minutes,
            input.cook_minutes,
            photo_path,
            input.notes,
            input.rating,
            input.youtube_url,
            input.source_url,
            n.calories,
            n.protein_g,
            n.carbs_g,
            n.fat_g,
            now,
        ],
    )?;

    tx.execute("DELETE FROM ingredients WHERE recipe_id = ?1", [&input.id])?;
    {
        let mut stmt = tx.prepare(
            "INSERT INTO ingredients
                (recipe_id, position, is_heading, quantity, quantity_max, unit, item, prep_note)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        )?;
        for (i, l) in recipe.ingredients.iter().enumerate() {
            stmt.execute(params![
                input.id,
                i as i64,
                l.is_heading,
                l.quantity,
                l.quantity_max,
                l.unit,
                l.item,
                l.prep_note
            ])?;
        }
    }

    tx.execute("DELETE FROM steps WHERE recipe_id = ?1", [&input.id])?;
    {
        let mut stmt =
            tx.prepare("INSERT INTO steps (recipe_id, position, text) VALUES (?1, ?2, ?3)")?;
        for (i, s) in input.steps.iter().enumerate() {
            stmt.execute(params![input.id, i as i64, s])?;
        }
    }

    labels::set_for_recipe(tx, LabelKind::Category, &input.id, &input.categories)?;
    labels::set_for_recipe(tx, LabelKind::Tag, &input.id, &input.tags)?;
    reindex(tx, recipe)
}

/// Deletes a recipe and returns the media paths it owned, or `None` if it didn't exist.
pub fn delete(conn: &mut Connection, id: &str) -> AppResult<Option<RecipeMedia>> {
    let tx = conn.transaction()?;
    let media = media_of(&tx, id)?;
    if media.is_some() {
        tx.execute("DELETE FROM recipes_fts WHERE recipe_id = ?1", [id])?;
        tx.execute("DELETE FROM recipes WHERE id = ?1", [id])?;
    }
    tx.commit()?;
    Ok(media)
}

/// All media paths referenced by any recipe (used to sweep orphaned files).
pub fn all_media_paths(conn: &Connection) -> AppResult<Vec<String>> {
    let mut stmt = conn.prepare(
        "SELECT photo_path FROM recipes WHERE photo_path IS NOT NULL
         UNION SELECT video_path FROM recipes WHERE video_path IS NOT NULL",
    )?;
    let paths = stmt
        .query_map([], |row| row.get(0))?
        .collect::<Result<Vec<String>, _>>()?;
    Ok(paths)
}

fn reindex(tx: &Transaction, recipe: &ValidRecipe) -> AppResult<()> {
    let input = &recipe.input;
    let ingredients = recipe
        .ingredients
        .iter()
        .map(|l| {
            [Some(l.item.as_str()), l.prep_note.as_deref()]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>()
                .join(" ")
        })
        .collect::<Vec<_>>()
        .join("\n");
    tx.execute("DELETE FROM recipes_fts WHERE recipe_id = ?1", [&input.id])?;
    tx.execute(
        "INSERT INTO recipes_fts (recipe_id, title, description, ingredients, steps, notes)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            input.id,
            input.title,
            input.description,
            ingredients,
            input.steps.join("\n"),
            input.notes
        ],
    )?;
    Ok(())
}

fn recipe_from_row(row: &Row) -> rusqlite::Result<Recipe> {
    Ok(Recipe {
        id: row.get(0)?,
        title: row.get(1)?,
        description: row.get(2)?,
        servings: row.get(3)?,
        prep_minutes: row.get(4)?,
        cook_minutes: row.get(5)?,
        photo_path: row.get(6)?,
        notes: row.get(7)?,
        rating: row.get(8)?,
        youtube_url: row.get(9)?,
        video_path: row.get(10)?,
        video_status: VideoStatus::parse(&row.get::<_, String>(11)?),
        source_url: row.get(12)?,
        nutrition: Nutrition {
            calories: row.get(13)?,
            protein_g: row.get(14)?,
            carbs_g: row.get(15)?,
            fat_g: row.get(16)?,
        },
        ingredients: Vec::new(),
        steps: Vec::new(),
        categories: Vec::new(),
        tags: Vec::new(),
        created_at: row.get(17)?,
        updated_at: row.get(18)?,
    })
}

fn ingredients_of(conn: &Connection, id: &str) -> AppResult<Vec<IngredientLine>> {
    let mut stmt = conn.prepare(
        "SELECT is_heading, quantity, quantity_max, unit, item, prep_note
         FROM ingredients WHERE recipe_id = ?1 ORDER BY position",
    )?;
    let lines = stmt
        .query_map([id], |row| {
            let (quantity, quantity_max) = (row.get(1)?, row.get(2)?);
            Ok(IngredientLine {
                is_heading: row.get(0)?,
                quantity,
                quantity_max,
                unit: row.get(3)?,
                item: row.get(4)?,
                prep_note: row.get(5)?,
                quantity_text: quantity::format(quantity, quantity_max),
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(lines)
}

fn steps_of(conn: &Connection, id: &str) -> AppResult<Vec<String>> {
    let mut stmt = conn.prepare("SELECT text FROM steps WHERE recipe_id = ?1 ORDER BY position")?;
    let steps = stmt
        .query_map([id], |row| row.get(0))?
        .collect::<Result<Vec<String>, _>>()?;
    Ok(steps)
}

fn categories_of(conn: &Connection, id: &str) -> AppResult<Vec<String>> {
    names_of(
        conn,
        "SELECT c.name FROM categories c JOIN recipe_categories rc ON rc.category_id = c.id
         WHERE rc.recipe_id = ?1 ORDER BY c.name COLLATE NOCASE",
        id,
    )
}

fn tags_of(conn: &Connection, id: &str) -> AppResult<Vec<String>> {
    names_of(
        conn,
        "SELECT t.name FROM tags t JOIN recipe_tags rt ON rt.tag_id = t.id
         WHERE rt.recipe_id = ?1 ORDER BY t.name COLLATE NOCASE",
        id,
    )
}

fn names_of(conn: &Connection, sql: &str, id: &str) -> AppResult<Vec<String>> {
    let mut stmt = conn.prepare_cached(sql)?;
    let names = stmt
        .query_map([id], |row| row.get(0))?
        .collect::<Result<Vec<String>, _>>()?;
    Ok(names)
}

/// Trims text and parses the quantity. Headings keep only their text.
fn clean_line(l: &IngredientInput) -> AppResult<IngredientLine> {
    let item = l.item.trim().to_string();
    if l.is_heading {
        return Ok(IngredientLine {
            is_heading: true,
            quantity: None,
            quantity_max: None,
            unit: None,
            item,
            prep_note: None,
            quantity_text: String::new(),
        });
    }
    let q = quantity::parse(&l.quantity).map_err(|()| {
        AppError::Validation(format!(
            "“{}” isn't a quantity Recipio understands. Try 2, 1.5, 1/2, 1 1/2 or 2-3.",
            l.quantity.trim()
        ))
    })?;
    let (quantity, quantity_max) = (q.map(|q| q.min), q.and_then(|q| q.max));
    Ok(IngredientLine {
        is_heading: false,
        quantity,
        quantity_max,
        unit: non_blank(l.unit.clone()),
        item,
        prep_note: non_blank(l.prep_note.clone()),
        quantity_text: quantity::format(quantity, quantity_max),
    })
}

fn non_blank(s: Option<String>) -> Option<String> {
    s.map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::open_in_memory;
    use crate::models::PhotoChange;

    fn line(q: &str, unit: Option<&str>, item: &str) -> IngredientInput {
        IngredientInput {
            is_heading: false,
            quantity: q.into(),
            unit: unit.map(Into::into),
            item: item.into(),
            prep_note: None,
        }
    }

    fn sample(id: &str) -> RecipeInput {
        RecipeInput {
            id: id.into(),
            title: "  Pancakes ".into(),
            description: "Fluffy".into(),
            servings: Some(4),
            prep_minutes: Some(10),
            cook_minutes: Some(15),
            photo: PhotoChange::Keep,
            notes: "Use buttermilk".into(),
            rating: Some(5),
            youtube_url: Some("  ".into()),
            source_url: None,
            nutrition: Nutrition {
                calories: Some(250.0),
                protein_g: None,
                carbs_g: None,
                fat_g: None,
            },
            ingredients: vec![
                IngredientInput {
                    is_heading: true,
                    quantity: "1".into(),
                    unit: None,
                    item: "Batter".into(),
                    prep_note: None,
                },
                line("1 1/2", Some("cups"), "flour"),
                line("", None, "  "),
                line("", None, "salt"),
            ],
            steps: vec!["Mix".into(), "   ".into(), "Cook".into()],
            categories: vec!["Breakfast".into()],
            tags: vec![" quick ".into(), "Quick".into()],
        }
    }

    const ID: &str = "6f1c1f0e-2b9a-4d4e-9a53-0d3f4b7d8c11";

    fn save(conn: &mut Connection, input: &RecipeInput, now: &str) {
        let clean = validate(input).unwrap();
        upsert(conn, &clean, Some("photos/x.jpg"), now).unwrap();
    }

    #[test]
    fn round_trip_cleans_input() {
        let mut conn = open_in_memory();
        save(&mut conn, &sample(ID), "2026-01-01T00:00:00Z");
        let r = get(&conn, ID).unwrap();
        assert_eq!(r.title, "Pancakes");
        assert_eq!(r.ingredients.len(), 3);
        assert!(r.ingredients[0].is_heading);
        assert_eq!(r.ingredients[0].quantity, None);
        assert_eq!(r.ingredients[1].quantity, Some(1.5));
        assert_eq!(r.ingredients[1].quantity_text, "1½");
        assert_eq!(r.ingredients[2].quantity_text, "");
        assert_eq!(r.steps, vec!["Mix", "Cook"]);
        assert_eq!(r.youtube_url, None);
        assert_eq!(r.photo_path.as_deref(), Some("photos/x.jpg"));
        assert_eq!(r.nutrition.calories, Some(250.0));
        assert_eq!(r.categories, vec!["Breakfast"]);
        assert_eq!(r.tags, vec!["quick"]);
    }

    #[test]
    fn update_keeps_created_at_and_replaces_children() {
        let mut conn = open_in_memory();
        save(&mut conn, &sample(ID), "2026-01-01T00:00:00Z");
        let mut changed = sample(ID);
        changed.ingredients = vec![line("2", None, "eggs")];
        changed.steps = vec!["Whisk".into()];
        save(&mut conn, &changed, "2026-02-01T00:00:00Z");
        let r = get(&conn, ID).unwrap();
        assert_eq!(r.created_at, "2026-01-01T00:00:00Z");
        assert_eq!(r.updated_at, "2026-02-01T00:00:00Z");
        assert_eq!(r.ingredients.len(), 1);
        assert_eq!(r.steps, vec!["Whisk"]);
        let fts: i64 = conn
            .query_row(
                "SELECT count(*) FROM recipes_fts WHERE recipes_fts MATCH 'whisk'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(fts, 1);
    }

    #[test]
    fn delete_returns_media_and_cascades() {
        let mut conn = open_in_memory();
        save(&mut conn, &sample(ID), "2026-01-01T00:00:00Z");
        let media = delete(&mut conn, ID).unwrap().unwrap();
        assert_eq!(media.photo_path.as_deref(), Some("photos/x.jpg"));
        assert!(matches!(get(&conn, ID), Err(AppError::NotFound(_))));
        let left: i64 = conn
            .query_row("SELECT count(*) FROM ingredients", [], |r| r.get(0))
            .unwrap();
        assert_eq!(left, 0);
        assert_eq!(delete(&mut conn, ID).unwrap(), None);
    }

    #[test]
    fn validation_rejects_bad_input() {
        let mut bad = sample(ID);
        bad.title = "   ".into();
        assert!(validate(&bad).is_err());
        let mut bad = sample(ID);
        bad.rating = Some(6);
        assert!(validate(&bad).is_err());
        let mut bad = sample("not-a-uuid");
        bad.title = "x".into();
        assert!(validate(&bad).is_err());
        let mut bad = sample(ID);
        bad.ingredients = vec![line("-1", None, "x")];
        assert!(validate(&bad).is_err());
        let mut bad = sample(ID);
        bad.ingredients = vec![line("a pinch", None, "salt")];
        assert!(matches!(validate(&bad), Err(AppError::Validation(m)) if m.contains("a pinch")));
    }

    #[test]
    fn quantities_are_parsed() {
        let l = clean_line(&line(" 2 - 3 ", None, "eggs")).unwrap();
        assert_eq!((l.quantity, l.quantity_max), (Some(2.0), Some(3.0)));
        assert_eq!(l.quantity_text, "2–3");
        let l = clean_line(&line("¾", Some(" cup "), "milk")).unwrap();
        assert_eq!((l.quantity, l.unit.as_deref()), (Some(0.75), Some("cup")));
    }

    fn query(search: &str) -> RecipeQuery {
        RecipeQuery {
            search: search.into(),
            category_ids: vec![],
            tag_ids: vec![],
            sort: RecipeSort::Title,
            descending: false,
        }
    }

    fn titles(conn: &Connection, q: &RecipeQuery) -> Vec<String> {
        list(conn, q)
            .unwrap()
            .into_iter()
            .map(|r| r.title)
            .collect()
    }

    /// Saves recipes "Apple pie", "banana bread", "Carrot soup" with distinct dates,
    /// ratings and labels.
    fn library(conn: &mut Connection) {
        let mk = |n: u8, title: &str, rating: Option<u8>, cats: &[&str], tags: &[&str]| {
            let mut r = sample(&format!("6f1c1f0e-2b9a-4d4e-9a53-0d3f4b7d8c1{n}"));
            r.title = title.into();
            r.rating = rating;
            r.notes = String::new();
            r.categories = cats.iter().map(|s| s.to_string()).collect();
            r.tags = tags.iter().map(|s| s.to_string()).collect();
            r
        };
        let recipes = [
            mk(1, "Apple pie", Some(3), &["Dessert"], &["baking"]),
            mk(
                2,
                "banana bread",
                None,
                &["Dessert", "Breakfast"],
                &["baking", "quick"],
            ),
            mk(3, "Carrot soup", Some(5), &["Dinner"], &["quick"]),
        ];
        for (i, r) in recipes.iter().enumerate() {
            save(conn, r, &format!("2026-01-0{}T00:00:00Z", i + 1));
        }
        // Make "updated" order differ from "created" order.
        let mut apple = recipes[0].clone();
        apple.description = "Updated".into();
        save(conn, &apple, "2026-03-01T00:00:00Z");
    }

    fn label_id(conn: &Connection, kind: LabelKind, name: &str) -> i64 {
        labels::list(conn, kind)
            .unwrap()
            .into_iter()
            .find(|l| l.name == name)
            .unwrap()
            .id
    }

    #[test]
    fn list_sorts_by_each_key() {
        let mut conn = open_in_memory();
        library(&mut conn);
        let mut q = query("");
        assert_eq!(
            titles(&conn, &q),
            ["Apple pie", "banana bread", "Carrot soup"]
        );
        q.descending = true;
        assert_eq!(
            titles(&conn, &q),
            ["Carrot soup", "banana bread", "Apple pie"]
        );
        q.sort = RecipeSort::Created;
        assert_eq!(
            titles(&conn, &q),
            ["Carrot soup", "banana bread", "Apple pie"]
        );
        q.sort = RecipeSort::Updated;
        assert_eq!(
            titles(&conn, &q),
            ["Apple pie", "Carrot soup", "banana bread"]
        );
        q.sort = RecipeSort::Rating;
        assert_eq!(
            titles(&conn, &q),
            ["Carrot soup", "Apple pie", "banana bread"]
        );
        q.descending = false;
        // Unrated stays last in both directions.
        assert_eq!(
            titles(&conn, &q),
            ["Apple pie", "Carrot soup", "banana bread"]
        );
    }

    #[test]
    fn list_filters_combine_with_and() {
        let mut conn = open_in_memory();
        library(&mut conn);
        let dessert = label_id(&conn, LabelKind::Category, "Dessert");
        let quick = label_id(&conn, LabelKind::Tag, "quick");
        let baking = label_id(&conn, LabelKind::Tag, "baking");
        let mut q = query("");
        q.category_ids = vec![dessert];
        assert_eq!(titles(&conn, &q), ["Apple pie", "banana bread"]);
        q.tag_ids = vec![quick];
        assert_eq!(titles(&conn, &q), ["banana bread"]);
        q.tag_ids = vec![quick, baking];
        assert_eq!(titles(&conn, &q), ["banana bread"]);
        q.search = "carrot".into();
        assert!(titles(&conn, &q).is_empty());
    }

    #[test]
    fn search_matches_prefixes_across_fields() {
        let mut conn = open_in_memory();
        library(&mut conn);
        // Every sample recipe has "flour" as an ingredient and "Mix" as a step.
        assert_eq!(titles(&conn, &query("flo")).len(), 3);
        assert_eq!(titles(&conn, &query("BAN")), ["banana bread"]);
        assert_eq!(titles(&conn, &query("updat mix")), ["Apple pie"]);
        assert!(titles(&conn, &query("apple carrot")).is_empty());
        // Punctuation and FTS syntax are treated as plain text, not errors.
        assert_eq!(titles(&conn, &query("\"soup")), ["Carrot soup"]);
        assert_eq!(titles(&conn, &query("soup OR NOT(")), Vec::<String>::new());
        assert_eq!(titles(&conn, &query(" - * ")).len(), 3);
    }

    #[test]
    fn fts_query_quotes_words() {
        assert_eq!(fts_query("  "), None);
        assert_eq!(fts_query("-- *"), None);
        assert_eq!(
            fts_query("choc \"chip\" crème-brûlée").as_deref(),
            Some("\"choc\"* \"chip\"* \"crème-brûlée\"*")
        );
    }

    #[test]
    fn records_and_checks_videos() {
        let mut conn = open_in_memory();
        let a = "00000000-0000-4000-8000-00000000000a";
        let b = "00000000-0000-4000-8000-00000000000b";
        assert!(!set_video(&conn, a, Some("videos/a.mp4"), VideoStatus::Ready, None).unwrap());
        save(&mut conn, &sample(a), "2026-01-01T00:00:00Z");
        save(&mut conn, &sample(b), "2026-01-01T00:00:00Z");

        let url = "https://youtu.be/x";
        let done = Some((url, "2026-02-01T00:00:00Z"));
        assert!(set_video(&conn, a, Some("videos/a.mp4"), VideoStatus::Ready, done).unwrap());
        let r = get(&conn, a).unwrap();
        assert_eq!(r.video_status, VideoStatus::Ready);
        assert_eq!(r.video_path.as_deref(), Some("videos/a.mp4"));
        assert_eq!(r.youtube_url.as_deref(), Some(url));
        assert_eq!(r.updated_at, "2026-02-01T00:00:00Z");
        assert!(
            list(&conn, &query(""))
                .unwrap()
                .iter()
                .any(|s| s.id == a && s.has_video)
        );

        // Saving the recipe again leaves the video alone.
        save(&mut conn, &sample(a), "2026-03-01T00:00:00Z");
        assert_eq!(get(&conn, a).unwrap().video_status, VideoStatus::Ready);

        set_video(&conn, b, None, VideoStatus::Downloading, None).unwrap();
        check_videos(&conn, |_| false).unwrap();
        assert_eq!(get(&conn, a).unwrap().video_status, VideoStatus::Missing);
        assert_eq!(get(&conn, a).unwrap().video_path, None);
        assert_eq!(get(&conn, b).unwrap().video_status, VideoStatus::Failed);
    }
}
