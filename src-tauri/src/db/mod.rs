pub mod bookmarks;
pub mod labels;
pub mod recipes;

use std::path::Path;

use rusqlite::Connection;
use rusqlite_migration::{M, Migrations};

use crate::error::AppResult;

fn migrations() -> Migrations<'static> {
    Migrations::new(vec![
        M::up(include_str!("../../migrations/0001_init.sql")),
        M::up(include_str!("../../migrations/0002_video_bookmarks.sql")),
    ])
}

/// Opens (creating if needed) the database file and applies pending migrations.
pub fn open(path: &Path) -> AppResult<Connection> {
    let mut conn = Connection::open(path)?;
    configure(&mut conn)?;
    Ok(conn)
}

fn configure(conn: &mut Connection) -> AppResult<()> {
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    migrations().to_latest(conn)?;
    Ok(())
}

#[cfg(test)]
pub fn open_in_memory() -> Connection {
    let mut conn = Connection::open_in_memory().unwrap();
    configure(&mut conn).unwrap();
    conn
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrations_are_valid() {
        migrations().validate().unwrap();
    }
}
