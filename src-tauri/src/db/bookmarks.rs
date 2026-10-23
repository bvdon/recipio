//! Video bookmarks: up to `MAX` named times per recipe, always listed in time order.

use rusqlite::{Connection, params};

use crate::duration;
use crate::error::{AppError, AppResult};
use crate::models::VideoBookmark;

pub const MAX: usize = 12;
const MAX_LABEL_CHARS: usize = 40;

pub fn list(conn: &Connection, recipe_id: &str) -> AppResult<Vec<VideoBookmark>> {
    let mut stmt = conn.prepare_cached(
        "SELECT id, seconds, label FROM video_bookmarks
         WHERE recipe_id = ?1 ORDER BY seconds, id",
    )?;
    let rows = stmt
        .query_map([recipe_id], |row| {
            let seconds: u32 = row.get(1)?;
            Ok(VideoBookmark {
                id: row.get(0)?,
                seconds,
                label: row.get(2)?,
                time_text: duration::clock(u64::from(seconds)),
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// Adds a bookmark. `duration` (the video's length in seconds, if known) bounds the time.
pub fn add(
    conn: &Connection,
    recipe_id: &str,
    seconds: u32,
    label: &str,
    duration: Option<f64>,
) -> AppResult<()> {
    let existing = list(conn, recipe_id)?;
    if existing.len() >= MAX {
        return Err(AppError::Validation(format!(
            "A video can have at most {MAX} bookmarks. Remove one to add another."
        )));
    }
    if let Some(d) = duration.filter(|d| d.is_finite() && *d > 0.0)
        && f64::from(seconds) > d.ceil()
    {
        return Err(AppError::Validation(format!(
            "That's past the end of the video ({}).",
            duration::clock(d as u64)
        )));
    }
    if let Some(b) = existing.iter().find(|b| b.seconds == seconds) {
        return Err(AppError::Validation(format!(
            "There's already a bookmark at {}.",
            b.time_text
        )));
    }
    let label = label.split_whitespace().collect::<Vec<_>>().join(" ");
    if label.chars().count() > MAX_LABEL_CHARS {
        return Err(AppError::Validation(format!(
            "Bookmark names can be at most {MAX_LABEL_CHARS} characters."
        )));
    }
    let n = conn.execute(
        "INSERT INTO video_bookmarks (recipe_id, seconds, label)
         SELECT id, ?2, ?3 FROM recipes WHERE id = ?1",
        params![recipe_id, seconds, label],
    )?;
    if n == 0 {
        return Err(AppError::NotFound("Recipe"));
    }
    Ok(())
}

pub fn delete(conn: &Connection, recipe_id: &str, id: i64) -> AppResult<()> {
    conn.execute(
        "DELETE FROM video_bookmarks WHERE recipe_id = ?1 AND id = ?2",
        params![recipe_id, id],
    )?;
    Ok(())
}

pub fn clear(conn: &Connection, recipe_id: &str) -> AppResult<()> {
    conn.execute(
        "DELETE FROM video_bookmarks WHERE recipe_id = ?1",
        [recipe_id],
    )?;
    Ok(())
}

/// Inserts already-checked bookmarks as they are (backup restore, shared recipes).
pub fn insert_all(conn: &Connection, recipe_id: &str, items: &[(u32, String)]) -> AppResult<()> {
    let mut stmt = conn.prepare_cached(
        "INSERT INTO video_bookmarks (recipe_id, seconds, label) VALUES (?1, ?2, ?3)",
    )?;
    for (seconds, label) in items.iter().take(MAX) {
        let label: String = label.trim().chars().take(MAX_LABEL_CHARS).collect();
        stmt.execute(params![recipe_id, seconds, label])?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::open_in_memory;

    const ID: &str = "11111111-1111-4111-8111-111111111111";

    fn setup() -> Connection {
        let conn = open_in_memory();
        conn.execute(
            "INSERT INTO recipes (id, title, created_at, updated_at) VALUES (?1, 'T', 'x', 'x')",
            [ID],
        )
        .unwrap();
        conn
    }

    #[test]
    fn keeps_bookmarks_in_time_order_with_limits() {
        let conn = setup();
        add(&conn, ID, 300, "  Sauce  time ", Some(600.0)).unwrap();
        add(&conn, ID, 15, "", None).unwrap();
        add(&conn, ID, 3725, "Plating", None).unwrap();
        let all = list(&conn, ID).unwrap();
        let times: Vec<_> = all.iter().map(|b| b.time_text.as_str()).collect();
        assert_eq!(times, vec!["0:15", "5:00", "1:02:05"]);
        assert_eq!(all[1].label, "Sauce time");

        assert!(
            add(&conn, ID, 300, "again", None).is_err(),
            "duplicate time"
        );
        let past = add(&conn, ID, 700, "", Some(600.0)).unwrap_err();
        assert!(past.user_message().contains("10:00"));
        assert!(add(&conn, ID, 1, &"x".repeat(41), None).is_err());

        for s in 100..109 {
            add(&conn, ID, s, "", None).unwrap();
        }
        assert_eq!(list(&conn, ID).unwrap().len(), MAX);
        assert!(
            add(&conn, ID, 2000, "", None)
                .unwrap_err()
                .user_message()
                .contains("at most 12")
        );

        let first = list(&conn, ID).unwrap()[0].id;
        delete(&conn, ID, first).unwrap();
        assert_eq!(list(&conn, ID).unwrap().len(), MAX - 1);
        clear(&conn, ID).unwrap();
        assert!(list(&conn, ID).unwrap().is_empty());
    }

    #[test]
    fn needs_a_saved_recipe() {
        let conn = open_in_memory();
        assert!(add(&conn, ID, 10, "", None).is_err());
    }
}
