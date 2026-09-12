use std::path::Path;
use rusqlite::{Connection, OpenFlags};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum DbError {
    #[error("Failed to open SQLite database at '{0}': {1}")]
    OpenFailed(String, rusqlite::Error),
    #[error("Failed to configure connection pragmas: {0}")]
    PragmaFailed(rusqlite::Error),
    #[error("Database file does not exist: {0}")]
    NotFound(String),
}

/// Opens a read-only SQLite connection configured for low-overhead WAL reads.
pub fn open_read_only(path: &Path) -> Result<Connection, DbError> {
    if !path.exists() {
        return Err(DbError::NotFound(path.display().to_string()));
    }

    let flags = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX;
    let conn = Connection::open_with_flags(path, flags)
        .map_err(|e| DbError::OpenFailed(path.display().to_string(), e))?;

    conn.execute_batch(
        "PRAGMA busy_timeout = 5000;
         PRAGMA query_only = ON;
         PRAGMA cache_size = -4000;
         PRAGMA mmap_size = 268435456;",
    )
    .map_err(DbError::PragmaFailed)?;

    Ok(conn)
}

/// Opens a read-write SQLite connection (used when creating fresh or test databases).
pub fn open_read_write(path: &Path) -> Result<Connection, DbError> {
    let conn = Connection::open(path)
        .map_err(|e| DbError::OpenFailed(path.display().to_string(), e))?;

    conn.execute_batch(
        "PRAGMA journal_mode = WAL;
         PRAGMA busy_timeout = 5000;
         PRAGMA synchronous = NORMAL;
         PRAGMA foreign_keys = ON;",
    )
    .map_err(DbError::PragmaFailed)?;

    Ok(conn)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_open_read_write_and_read_only() {
        let temp = tempfile::NamedTempFile::new().unwrap();
        let conn_rw = open_read_write(temp.path()).unwrap();
        conn_rw
            .execute("CREATE TABLE test (id INTEGER PRIMARY KEY, name TEXT);", [])
            .unwrap();
        conn_rw
            .execute("INSERT INTO test (name) VALUES ('alpha');", [])
            .unwrap();
        drop(conn_rw);

        let conn_ro = open_read_only(temp.path()).unwrap();
        let name: String = conn_ro
            .query_row("SELECT name FROM test WHERE id = 1", [], |r| r.get(0))
            .unwrap();
        assert_eq!(name, "alpha");

        // Verifying query_only prevents writes
        let write_res = conn_ro.execute("INSERT INTO test (name) VALUES ('beta');", []);
        assert!(write_res.is_err());
    }
}
