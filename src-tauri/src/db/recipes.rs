use rusqlite::{Connection, OptionalExtension, Row, Transaction, params};

use crate::error::{AppError, AppResult};
use crate::models::{IngredientLine, Nutrition, Recipe, RecipeInput, RecipeSummary, VideoStatus};

/// Media files owned by a recipe, relative to the media folder.
#[derive(Debug, Default, PartialEq)]
pub struct RecipeMedia {
    pub photo_path: Option<String>,
    pub video_path: Option<String>,
}

pub fn list(conn: &Connection) -> AppResult<Vec<RecipeSummary>> {
    let mut stmt = conn.prepare(
        "SELECT id, title, photo_path, rating, video_status, created_at, updated_at
         FROM recipes ORDER BY updated_at DESC",
    )?;
    let mut summaries = stmt
        .query_map([], |row| {
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

/// Checks the input and returns a cleaned copy (trimmed text, blank lines removed).
pub fn validate(input: &RecipeInput) -> AppResult<RecipeInput> {
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
    for q in input
        .ingredients
        .iter()
        .flat_map(|l| [l.quantity, l.quantity_max])
        .flatten()
    {
        if !q.is_finite() || q < 0.0 {
            return Err(AppError::Validation(
                "Ingredient quantities must be positive numbers.".into(),
            ));
        }
    }
    input.ingredients = input
        .ingredients
        .into_iter()
        .map(clean_line)
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
    Ok(input)
}

/// Inserts or updates a recipe (by id) together with its ingredients, steps and search
/// index entry. `input` must already be validated. Video fields are left untouched.
pub fn upsert(
    conn: &mut Connection,
    input: &RecipeInput,
    photo_path: Option<&str>,
    now: &str,
) -> AppResult<()> {
    let tx = conn.transaction()?;
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
        for (i, l) in input.ingredients.iter().enumerate() {
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

    reindex(&tx, input)?;
    tx.commit()?;
    Ok(())
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

fn reindex(tx: &Transaction, input: &RecipeInput) -> AppResult<()> {
    let ingredients = input
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
            Ok(IngredientLine {
                is_heading: row.get(0)?,
                quantity: row.get(1)?,
                quantity_max: row.get(2)?,
                unit: row.get(3)?,
                item: row.get(4)?,
                prep_note: row.get(5)?,
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

fn clean_line(mut l: IngredientLine) -> IngredientLine {
    l.item = l.item.trim().to_string();
    l.unit = non_blank(l.unit);
    l.prep_note = non_blank(l.prep_note);
    if l.is_heading {
        l.quantity = None;
        l.quantity_max = None;
        l.unit = None;
        l.prep_note = None;
    }
    // A max without a min, or a max not above the min, is not a range.
    match (l.quantity, l.quantity_max) {
        (None, Some(max)) => {
            l.quantity = Some(max);
            l.quantity_max = None;
        }
        (Some(min), Some(max)) if max <= min => l.quantity_max = None,
        _ => {}
    }
    l
}

fn non_blank(s: Option<String>) -> Option<String> {
    s.map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::open_in_memory;
    use crate::models::PhotoChange;

    fn line(q: Option<f64>, unit: Option<&str>, item: &str) -> IngredientLine {
        IngredientLine {
            is_heading: false,
            quantity: q,
            quantity_max: None,
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
                IngredientLine {
                    is_heading: true,
                    quantity: Some(1.0),
                    quantity_max: None,
                    unit: None,
                    item: "Batter".into(),
                    prep_note: None,
                },
                line(Some(1.5), Some("cups"), "flour"),
                line(None, None, "  "),
                line(None, None, "salt"),
            ],
            steps: vec!["Mix".into(), "   ".into(), "Cook".into()],
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
        assert_eq!(r.steps, vec!["Mix", "Cook"]);
        assert_eq!(r.youtube_url, None);
        assert_eq!(r.photo_path.as_deref(), Some("photos/x.jpg"));
        assert_eq!(r.nutrition.calories, Some(250.0));
    }

    #[test]
    fn update_keeps_created_at_and_replaces_children() {
        let mut conn = open_in_memory();
        save(&mut conn, &sample(ID), "2026-01-01T00:00:00Z");
        let mut changed = sample(ID);
        changed.ingredients = vec![line(Some(2.0), None, "eggs")];
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
        bad.ingredients = vec![line(Some(-1.0), None, "x")];
        assert!(validate(&bad).is_err());
    }

    #[test]
    fn ranges_are_normalized() {
        let mut l = line(Some(3.0), None, "eggs");
        l.quantity_max = Some(2.0);
        assert_eq!(clean_line(l).quantity_max, None);
        let mut l = line(None, None, "eggs");
        l.quantity_max = Some(2.0);
        let l = clean_line(l);
        assert_eq!((l.quantity, l.quantity_max), (Some(2.0), None));
    }
}
