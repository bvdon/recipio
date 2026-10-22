use serde::ser::SerializeStruct;
use serde::{Serialize, Serializer};

/// Error type returned by every Tauri command.
///
/// The frontend receives `{ kind, message }` where `message` is plain language.
/// Technical detail is written to the log file when the error is serialized.
#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("{0}")]
    Validation(String),
    #[error("{0} not found")]
    NotFound(&'static str),
    #[error("database error: {0}")]
    Database(#[from] rusqlite::Error),
    #[error("migration error: {0}")]
    Migration(#[from] rusqlite_migration::Error),
    #[error("file error: {0}")]
    Io(#[from] std::io::Error),
}

pub type AppResult<T> = Result<T, AppError>;

impl AppError {
    fn kind(&self) -> &'static str {
        match self {
            AppError::Validation(_) => "validation",
            AppError::NotFound(_) => "not_found",
            AppError::Database(_) | AppError::Migration(_) => "database",
            AppError::Io(_) => "io",
        }
    }

    fn user_message(&self) -> String {
        match self {
            AppError::Validation(_) | AppError::NotFound(_) => self.to_string(),
            AppError::Database(_) | AppError::Migration(_) => {
                "Something went wrong reading or writing the recipe database. Details are in the log file.".into()
            }
            AppError::Io(_) => {
                "A file could not be read or written. Details are in the log file.".into()
            }
        }
    }
}

impl Serialize for AppError {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        // Serialization happens exactly once, when the error crosses to the frontend,
        // so this is the single place where command failures get logged.
        log::error!("{self:?}");
        let mut s = serializer.serialize_struct("AppError", 2)?;
        s.serialize_field("kind", self.kind())?;
        s.serialize_field("message", &self.user_message())?;
        s.end()
    }
}
