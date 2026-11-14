use std::fmt::Display;

#[derive(Debug)]
pub enum Error {
    Sqlite(rusqlite::Error),
    Migration(rusqlite_migration::Error),
}

impl Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Sqlite(e) => writeln!(f, "storage error: {e}"),
            Self::Migration(e) => writeln!(f, "migration error: {e}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<rusqlite::Error> for Error {
    fn from(value: rusqlite::Error) -> Self {
        Self::Sqlite(value)
    }
}

impl From<rusqlite_migration::Error> for Error {
    fn from(value: rusqlite_migration::Error) -> Self {
        Self::Migration(value)
    }
}
