//! Categories and tags. Both are flat, uniquely named (case-insensitive) labels with a
//! many-to-many link to recipes, so one implementation serves both via `LabelKind`.

use rusqlite::{Connection, OptionalExtension, Transaction, params, params_from_iter};

use crate::error::{AppError, AppResult};
use crate::models::{Label, LabelKind};

const MAX_NAME_CHARS: usize = 60;

struct Tables {
    labels: &'static str,
    links: &'static str,
    fk: &'static str,
    noun: &'static str,
}

fn tables(kind: LabelKind) -> Tables {
    match kind {
        LabelKind::Category => Tables {
            labels: "categories",
            links: "recipe_categories",
            fk: "category_id",
            noun: "category",
        },
        LabelKind::Tag => Tables {
            labels: "tags",
            links: "recipe_tags",
            fk: "tag_id",
            noun: "tag",
        },
    }
}

/// Trims and collapses whitespace. Returns an error for blank or overlong names.
pub fn clean_name(kind: LabelKind, name: &str) -> AppResult<String> {
    let name = name.split_whitespace().collect::<Vec<_>>().join(" ");
    let noun = tables(kind).noun;
    if name.is_empty() {
        return Err(AppError::Validation(format!(
            "The {noun} name can't be empty."
        )));
    }
    if name.chars().count() > MAX_NAME_CHARS {
        return Err(AppError::Validation(format!(
            "The {noun} name “{name}” is too long (max {MAX_NAME_CHARS} characters)."
        )));
    }
    Ok(name)
}

/// Cleans a list of names and drops case-insensitive duplicates, keeping the first.
pub fn clean_names(kind: LabelKind, names: &[String]) -> AppResult<Vec<String>> {
    let mut out: Vec<String> = Vec::new();
    for name in names {
        let name = clean_name(kind, name)?;
        if !out.iter().any(|n| n.to_lowercase() == name.to_lowercase()) {
            out.push(name);
        }
    }
    Ok(out)
}

/// All labels of a kind, alphabetical, with recipe counts.
pub fn list(conn: &Connection, kind: LabelKind) -> AppResult<Vec<Label>> {
    let t = tables(kind);
    let mut stmt = conn.prepare(&format!(
        "SELECT l.id, l.name, count(x.recipe_id)
         FROM {labels} l LEFT JOIN {links} x ON x.{fk} = l.id
         GROUP BY l.id ORDER BY l.name COLLATE NOCASE",
        labels = t.labels,
        links = t.links,
        fk = t.fk,
    ))?;
    let labels = stmt
        .query_map([], |row| {
            Ok(Label {
                id: row.get(0)?,
                name: row.get(1)?,
                recipe_count: row.get(2)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(labels)
}

/// Creates a label, or returns the existing one with the same name (any case).
pub fn create(conn: &Connection, kind: LabelKind, name: &str) -> AppResult<i64> {
    let name = clean_name(kind, name)?;
    ensure(conn, kind, &name)
}

fn ensure(conn: &Connection, kind: LabelKind, name: &str) -> AppResult<i64> {
    let t = tables(kind);
    conn.prepare_cached(&format!(
        "INSERT INTO {} (name) VALUES (?1) ON CONFLICT(name) DO NOTHING",
        t.labels
    ))?
    .execute([name])?;
    let id = conn
        .prepare_cached(&format!("SELECT id FROM {} WHERE name = ?1", t.labels))?
        .query_row([name], |row| row.get(0))?;
    Ok(id)
}

pub fn rename(conn: &Connection, kind: LabelKind, id: i64, name: &str) -> AppResult<()> {
    let t = tables(kind);
    let name = clean_name(kind, name)?;
    let clash: Option<String> = conn
        .query_row(
            &format!("SELECT name FROM {} WHERE name = ?1 AND id != ?2", t.labels),
            params![name, id],
            |row| row.get(0),
        )
        .optional()?;
    if let Some(existing) = clash {
        return Err(AppError::Validation(format!(
            "A {} named “{existing}” already exists. Use Merge to combine them.",
            t.noun
        )));
    }
    let changed = conn.execute(
        &format!("UPDATE {} SET name = ?1 WHERE id = ?2", t.labels),
        params![name, id],
    )?;
    if changed == 0 {
        return Err(AppError::NotFound(label_noun(kind)));
    }
    Ok(())
}

/// Deletes a label and its links. Recipes themselves are untouched.
pub fn delete(conn: &Connection, kind: LabelKind, id: i64) -> AppResult<()> {
    let t = tables(kind);
    let changed = conn.execute(&format!("DELETE FROM {} WHERE id = ?1", t.labels), [id])?;
    if changed == 0 {
        return Err(AppError::NotFound(label_noun(kind)));
    }
    Ok(())
}

/// Moves every recipe from `sources` onto `target`, then deletes the sources.
pub fn merge(
    conn: &mut Connection,
    kind: LabelKind,
    sources: &[i64],
    target: i64,
) -> AppResult<()> {
    let t = tables(kind);
    let sources: Vec<i64> = sources.iter().copied().filter(|&s| s != target).collect();
    if sources.is_empty() {
        return Err(AppError::Validation(format!(
            "Choose at least one other {} to merge.",
            t.noun
        )));
    }
    let tx = conn.transaction()?;
    let exists: Option<i64> = tx
        .query_row(
            &format!("SELECT id FROM {} WHERE id = ?1", t.labels),
            [target],
            |row| row.get(0),
        )
        .optional()?;
    if exists.is_none() {
        return Err(AppError::NotFound(label_noun(kind)));
    }
    let marks = vec!["?"; sources.len()].join(", ");
    tx.execute(
        &format!(
            "INSERT OR IGNORE INTO {links} (recipe_id, {fk})
             SELECT recipe_id, ? FROM {links} WHERE {fk} IN ({marks})",
            links = t.links,
            fk = t.fk,
        ),
        params_from_iter(std::iter::once(target).chain(sources.iter().copied())),
    )?;
    tx.execute(
        &format!("DELETE FROM {} WHERE id IN ({marks})", t.labels),
        params_from_iter(&sources),
    )?;
    tx.commit()?;
    Ok(())
}

/// Replaces a recipe's labels with `names` (already cleaned), creating unknown ones.
pub fn set_for_recipe(
    tx: &Transaction,
    kind: LabelKind,
    recipe_id: &str,
    names: &[String],
) -> AppResult<()> {
    let t = tables(kind);
    tx.execute(
        &format!("DELETE FROM {} WHERE recipe_id = ?1", t.links),
        [recipe_id],
    )?;
    for name in names {
        let id = ensure(tx, kind, name)?;
        tx.prepare_cached(&format!(
            "INSERT OR IGNORE INTO {} (recipe_id, {}) VALUES (?1, ?2)",
            t.links, t.fk
        ))?
        .execute(params![recipe_id, id])?;
    }
    Ok(())
}

fn label_noun(kind: LabelKind) -> &'static str {
    match kind {
        LabelKind::Category => "Category",
        LabelKind::Tag => "Tag",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::open_in_memory;

    fn add_recipe(conn: &Connection, id: &str) {
        conn.execute(
            "INSERT INTO recipes (id, title, created_at, updated_at) VALUES (?1, ?1, '', '')",
            [id],
        )
        .unwrap();
    }

    fn link(conn: &mut Connection, recipe: &str, kind: LabelKind, names: &[&str]) {
        let names: Vec<String> = names.iter().map(|s| s.to_string()).collect();
        let tx = conn.transaction().unwrap();
        set_for_recipe(&tx, kind, recipe, &names).unwrap();
        tx.commit().unwrap();
    }

    fn names(conn: &Connection, kind: LabelKind) -> Vec<(String, u32)> {
        list(conn, kind)
            .unwrap()
            .into_iter()
            .map(|l| (l.name, l.recipe_count))
            .collect()
    }

    #[test]
    fn names_are_cleaned_and_deduplicated() {
        let input = vec![
            "  Gluten   free ".into(),
            "gluten FREE".into(),
            "Quick".into(),
        ];
        assert_eq!(
            clean_names(LabelKind::Tag, &input).unwrap(),
            vec!["Gluten free", "Quick"]
        );
        assert!(clean_name(LabelKind::Tag, "   ").is_err());
        assert!(clean_name(LabelKind::Tag, &"x".repeat(61)).is_err());
    }

    #[test]
    fn linking_reuses_existing_names_case_insensitively() {
        let mut conn = open_in_memory();
        add_recipe(&conn, "a");
        add_recipe(&conn, "b");
        link(&mut conn, "a", LabelKind::Category, &["Dessert"]);
        link(&mut conn, "b", LabelKind::Category, &["dessert", "Baking"]);
        assert_eq!(
            names(&conn, LabelKind::Category),
            vec![("Baking".into(), 1), ("Dessert".into(), 2)]
        );
        // Re-linking replaces; the now-unused label stays with a count of 0.
        link(&mut conn, "b", LabelKind::Category, &["Dessert"]);
        assert_eq!(
            names(&conn, LabelKind::Category),
            vec![("Baking".into(), 0), ("Dessert".into(), 2)]
        );
    }

    #[test]
    fn rename_refuses_clashes_but_allows_case_changes() {
        let conn = open_in_memory();
        let a = create(&conn, LabelKind::Tag, "vegan").unwrap();
        create(&conn, LabelKind::Tag, "Quick").unwrap();
        assert!(matches!(
            rename(&conn, LabelKind::Tag, a, "quick"),
            Err(AppError::Validation(_))
        ));
        rename(&conn, LabelKind::Tag, a, "Vegan").unwrap();
        assert_eq!(
            names(&conn, LabelKind::Tag),
            vec![("Quick".into(), 0), ("Vegan".into(), 0)]
        );
        assert!(matches!(
            rename(&conn, LabelKind::Tag, 999, "x"),
            Err(AppError::NotFound(_))
        ));
    }

    #[test]
    fn create_returns_existing_id() {
        let conn = open_in_memory();
        let a = create(&conn, LabelKind::Category, "Soup").unwrap();
        assert_eq!(create(&conn, LabelKind::Category, " soup ").unwrap(), a);
    }

    #[test]
    fn merge_moves_links_without_duplicates() {
        let mut conn = open_in_memory();
        for id in ["a", "b", "c"] {
            add_recipe(&conn, id);
        }
        link(&mut conn, "a", LabelKind::Tag, &["GF"]);
        link(&mut conn, "b", LabelKind::Tag, &["GF", "Gluten-free"]);
        link(&mut conn, "c", LabelKind::Tag, &["No gluten"]);
        let ids: Vec<i64> = list(&conn, LabelKind::Tag)
            .unwrap()
            .iter()
            .map(|l| l.id)
            .collect();
        let [gf, gluten_free, no_gluten] = ids[..] else {
            panic!()
        };
        merge(
            &mut conn,
            LabelKind::Tag,
            &[gf, no_gluten, gluten_free],
            gluten_free,
        )
        .unwrap();
        assert_eq!(
            names(&conn, LabelKind::Tag),
            vec![("Gluten-free".into(), 3)]
        );
        assert!(merge(&mut conn, LabelKind::Tag, &[gluten_free], gluten_free).is_err());
    }

    #[test]
    fn delete_unlinks_but_keeps_recipes() {
        let mut conn = open_in_memory();
        add_recipe(&conn, "a");
        link(&mut conn, "a", LabelKind::Category, &["Dinner"]);
        let id = list(&conn, LabelKind::Category).unwrap()[0].id;
        delete(&conn, LabelKind::Category, id).unwrap();
        let links: i64 = conn
            .query_row("SELECT count(*) FROM recipe_categories", [], |r| r.get(0))
            .unwrap();
        let recipes: i64 = conn
            .query_row("SELECT count(*) FROM recipes", [], |r| r.get(0))
            .unwrap();
        assert_eq!((links, recipes), (0, 1));
        assert!(delete(&conn, LabelKind::Category, id).is_err());
    }
}
