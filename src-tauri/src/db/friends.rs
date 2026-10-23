//! Friends (who recipes are sent to, and how), the local log of shared files, and small
//! app settings such as the user's own name.

use rusqlite::{Connection, OptionalExtension, params};

use crate::error::{AppError, AppResult};
use crate::models::{Friend, FriendInput, ShareDirection, ShareLogEntry, ShareMethod};

const MAX_NAME_CHARS: usize = 60;
/// Settings key for the name shown to friends in shared files.
pub const MY_NAME: &str = "my_name";

pub fn list(conn: &Connection) -> AppResult<Vec<Friend>> {
    let mut stmt = conn.prepare(
        "SELECT id, name, method, address, created_at FROM friends ORDER BY name COLLATE NOCASE, id",
    )?;
    let rows = stmt
        .query_map([], |row| {
            Ok(Friend {
                id: row.get(0)?,
                name: row.get(1)?,
                method: ShareMethod::parse(&row.get::<_, String>(2)?),
                address: row.get(3)?,
                created_at: row.get(4)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

pub fn get(conn: &Connection, id: i64) -> AppResult<Friend> {
    list(conn)?
        .into_iter()
        .find(|f| f.id == id)
        .ok_or(AppError::NotFound("Friend"))
}

/// Cleans and checks a friend's details: a name, and a phone number or email address
/// (Messages accepts either; email needs an email address).
pub fn validate(input: &FriendInput) -> AppResult<FriendInput> {
    let name = clean_name(&input.name)?;
    let address = input.address.trim();
    let is_email = address.contains('@')
        && address
            .rsplit_once('@')
            .is_some_and(|(user, domain)| !user.is_empty() && domain.contains('.'))
        && !address.contains(char::is_whitespace);
    let phone: String = address
        .chars()
        .filter(|c| c.is_ascii_digit() || *c == '+')
        .collect();
    let is_phone = !address.contains('@')
        && phone.chars().filter(char::is_ascii_digit).count() >= 5
        && address
            .chars()
            .all(|c| c.is_ascii_digit() || " +-().".contains(c));
    let address = match input.method {
        ShareMethod::Email if is_email => address.to_string(),
        ShareMethod::Email => {
            return Err(AppError::Validation(
                "Please enter an email address, like sam@example.com.".into(),
            ));
        }
        ShareMethod::Messages if is_email => address.to_string(),
        ShareMethod::Messages if is_phone => phone,
        ShareMethod::Messages => {
            return Err(AppError::Validation(
                "Please enter the phone number or email they use for Messages.".into(),
            ));
        }
    };
    Ok(FriendInput {
        name,
        method: input.method,
        address,
    })
}

fn clean_name(name: &str) -> AppResult<String> {
    let name = name.split_whitespace().collect::<Vec<_>>().join(" ");
    if name.is_empty() {
        return Err(AppError::Validation("Please enter a name.".into()));
    }
    if name.chars().count() > MAX_NAME_CHARS {
        return Err(AppError::Validation(format!(
            "Names can be at most {MAX_NAME_CHARS} characters."
        )));
    }
    Ok(name)
}

pub fn add(conn: &Connection, input: &FriendInput, now: &str) -> AppResult<Friend> {
    let f = validate(input)?;
    conn.execute(
        "INSERT INTO friends (name, method, address, created_at) VALUES (?1, ?2, ?3, ?4)",
        params![f.name, f.method.as_str(), f.address, now],
    )?;
    get(conn, conn.last_insert_rowid())
}

pub fn update(conn: &Connection, id: i64, input: &FriendInput) -> AppResult<Friend> {
    let f = validate(input)?;
    let n = conn.execute(
        "UPDATE friends SET name = ?2, method = ?3, address = ?4 WHERE id = ?1",
        params![id, f.name, f.method.as_str(), f.address],
    )?;
    if n == 0 {
        return Err(AppError::NotFound("Friend"));
    }
    get(conn, id)
}

/// Removes the friend; their history stays (without the link).
pub fn delete(conn: &Connection, id: i64) -> AppResult<()> {
    conn.execute("DELETE FROM friends WHERE id = ?1", [id])?;
    Ok(())
}

/// The friend whose name matches a sender's name (for received files).
pub fn find_by_name(conn: &Connection, name: &str) -> AppResult<Option<Friend>> {
    let name = name.trim();
    Ok(list(conn)?
        .into_iter()
        .find(|f| f.name.eq_ignore_ascii_case(name)))
}

pub fn log(
    conn: &Connection,
    friend_id: Option<i64>,
    direction: ShareDirection,
    person: &str,
    titles: &[String],
    file_name: &str,
    at: &str,
) -> AppResult<()> {
    let dir = match direction {
        ShareDirection::Sent => "sent",
        ShareDirection::Received => "received",
    };
    let titles = serde_json::to_string(titles).unwrap_or_else(|_| "[]".into());
    conn.execute(
        "INSERT INTO share_log (friend_id, direction, person, titles, file_name, at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![friend_id, dir, person, titles, file_name, at],
    )?;
    Ok(())
}

/// History, newest first; only one friend's when `friend_id` is given.
pub fn history(conn: &Connection, friend_id: Option<i64>) -> AppResult<Vec<ShareLogEntry>> {
    let mut stmt = conn.prepare(
        "SELECT id, friend_id, direction, person, titles, file_name, at FROM share_log
         WHERE ?1 IS NULL OR friend_id = ?1 ORDER BY at DESC, id DESC",
    )?;
    let rows = stmt
        .query_map([friend_id], |row| {
            let titles: String = row.get(4)?;
            Ok(ShareLogEntry {
                id: row.get(0)?,
                friend_id: row.get(1)?,
                direction: if row.get::<_, String>(2)? == "received" {
                    ShareDirection::Received
                } else {
                    ShareDirection::Sent
                },
                person: row.get(3)?,
                titles: serde_json::from_str(&titles).unwrap_or_default(),
                file_name: row.get(5)?,
                at: row.get(6)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

pub fn get_setting(conn: &Connection, key: &str) -> AppResult<Option<String>> {
    Ok(conn
        .query_row("SELECT value FROM settings WHERE key = ?1", [key], |row| {
            row.get(0)
        })
        .optional()?)
}

pub fn set_setting(conn: &Connection, key: &str, value: &str) -> AppResult<()> {
    conn.execute(
        "INSERT INTO settings (key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![key, value],
    )?;
    Ok(())
}

/// The user's name for shared files ("" if not set yet).
pub fn my_name(conn: &Connection) -> AppResult<String> {
    Ok(get_setting(conn, MY_NAME)?.unwrap_or_default())
}

pub fn set_my_name(conn: &Connection, name: &str) -> AppResult<String> {
    let name = clean_name(name)?;
    set_setting(conn, MY_NAME, &name)?;
    Ok(name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::open_in_memory;

    fn input(name: &str, method: ShareMethod, address: &str) -> FriendInput {
        FriendInput {
            name: name.into(),
            method,
            address: address.into(),
        }
    }

    #[test]
    fn checks_friend_details() {
        let ok = |n, m, a| validate(&input(n, m, a)).map(|f| (f.name, f.address));
        assert_eq!(
            ok("  Sam   Lee ", ShareMethod::Messages, "+1 (555) 123-4567").unwrap(),
            ("Sam Lee".into(), "+15551234567".into())
        );
        assert_eq!(
            ok("Sam", ShareMethod::Messages, "sam@icloud.com")
                .unwrap()
                .1,
            "sam@icloud.com"
        );
        assert_eq!(
            ok("Ana", ShareMethod::Email, " ana@example.com ")
                .unwrap()
                .1,
            "ana@example.com"
        );
        assert!(ok("Ana", ShareMethod::Email, "555 1234").is_err());
        assert!(ok("Ana", ShareMethod::Email, "ana@localhost").is_err());
        assert!(ok("Sam", ShareMethod::Messages, "call me").is_err());
        assert!(ok("Sam", ShareMethod::Messages, "123").is_err());
        assert!(ok("   ", ShareMethod::Email, "a@b.co").is_err());
    }

    #[test]
    fn stores_friends_history_and_settings() {
        let conn = open_in_memory();
        let now = "2026-10-23T10:00:00Z";
        let sam = add(
            &conn,
            &input("Sam", ShareMethod::Messages, "555-123-4567"),
            now,
        )
        .unwrap();
        let ana = add(
            &conn,
            &input("ana", ShareMethod::Email, "ana@example.com"),
            now,
        )
        .unwrap();
        let names: Vec<_> = list(&conn).unwrap().into_iter().map(|f| f.name).collect();
        assert_eq!(names, vec!["ana", "Sam"], "alphabetical, ignoring case");

        let sam = update(
            &conn,
            sam.id,
            &input("Sam Lee", ShareMethod::Email, "sam@x.org"),
        )
        .unwrap();
        assert_eq!(
            (sam.method, sam.address.as_str()),
            (ShareMethod::Email, "sam@x.org")
        );
        assert_eq!(find_by_name(&conn, " ANA ").unwrap().unwrap().id, ana.id);

        log(
            &conn,
            Some(sam.id),
            ShareDirection::Sent,
            "Sam Lee",
            &["Curry".into()],
            "a.recipio",
            "2026-10-23T10:00:00Z",
        )
        .unwrap();
        log(
            &conn,
            Some(sam.id),
            ShareDirection::Received,
            "Sam Lee",
            &["Pie".into(), "Soup".into()],
            "b.recipio",
            "2026-10-23T11:00:00Z",
        )
        .unwrap();
        log(
            &conn,
            Some(ana.id),
            ShareDirection::Sent,
            "ana",
            &[],
            "c.recipio",
            "2026-10-23T12:00:00Z",
        )
        .unwrap();
        let sams = history(&conn, Some(sam.id)).unwrap();
        assert_eq!(sams.len(), 2);
        assert_eq!(sams[0].direction, ShareDirection::Received, "newest first");
        assert_eq!(sams[0].titles, vec!["Pie", "Soup"]);
        assert_eq!(history(&conn, None).unwrap().len(), 3);

        delete(&conn, sam.id).unwrap();
        assert_eq!(list(&conn).unwrap().len(), 1);
        let kept = history(&conn, None).unwrap();
        assert_eq!(kept.len(), 3, "history is kept");
        assert!(
            kept.iter()
                .filter(|e| e.person == "Sam Lee")
                .all(|e| e.friend_id.is_none())
        );

        assert_eq!(my_name(&conn).unwrap(), "");
        assert_eq!(set_my_name(&conn, "  Rob  H ").unwrap(), "Rob H");
        assert_eq!(my_name(&conn).unwrap(), "Rob H");
        assert!(set_my_name(&conn, " ").is_err());
    }
}
