pub use rusqlite::Connection;
use rusqlite::OpenFlags;
use std::path::Path;
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
    let conn =
        Connection::open(path).map_err(|e| DbError::OpenFailed(path.display().to_string(), e))?;

    conn.execute_batch(
        "PRAGMA journal_mode = WAL;
         PRAGMA busy_timeout = 5000;
         PRAGMA synchronous = NORMAL;
         PRAGMA foreign_keys = ON;",
    )
    .map_err(DbError::PragmaFailed)?;

    Ok(conn)
}

/// Ensures the `symbols_fts` FTS5 virtual table and synchronization triggers exist in the SQLite database.
/// If `symbols` has rows but `symbols_fts` has not indexed them (e.g. freshly created FTS table),
/// an index rebuild is executed.
pub fn ensure_fts_index(conn: &Connection) -> Result<(), rusqlite::Error> {
    let symbols_table_exists: bool = conn
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type='table' AND name='symbols'",
            [],
            |_| Ok(true),
        )
        .unwrap_or(false);

    if !symbols_table_exists {
        return Ok(());
    }

    conn.execute_batch(
        "CREATE VIRTUAL TABLE IF NOT EXISTS symbols_fts USING fts5(
            name,
            signature,
            doc_comment,
            content='symbols',
            content_rowid='rowid',
            tokenize='porter unicode61'
        );

        CREATE TRIGGER IF NOT EXISTS symbols_ai AFTER INSERT ON symbols BEGIN
            INSERT INTO symbols_fts(rowid, name, signature, doc_comment)
            VALUES (new.rowid, new.name, new.signature, new.doc_comment);
        END;

        CREATE TRIGGER IF NOT EXISTS symbols_ad AFTER DELETE ON symbols BEGIN
            INSERT INTO symbols_fts(symbols_fts, rowid, name, signature, doc_comment)
            VALUES ('delete', old.rowid, old.name, old.signature, old.doc_comment);
        END;

        CREATE TRIGGER IF NOT EXISTS symbols_au AFTER UPDATE ON symbols BEGIN
            INSERT INTO symbols_fts(symbols_fts, rowid, name, signature, doc_comment)
            VALUES ('delete', old.rowid, old.name, old.signature, old.doc_comment);
            INSERT INTO symbols_fts(rowid, name, signature, doc_comment)
            VALUES (new.rowid, new.name, new.signature, new.doc_comment);
        END;",
    )?;

    let symbol_count: i64 = conn
        .query_row("SELECT count(*) FROM symbols", [], |r| r.get(0))
        .unwrap_or(0);
    let docsize_count: i64 = conn
        .query_row("SELECT count(*) FROM symbols_fts_docsize", [], |r| r.get(0))
        .unwrap_or(0);

    if symbol_count > 0 && docsize_count == 0 {
        conn.execute("INSERT INTO symbols_fts(symbols_fts) VALUES('rebuild')", [])?;
    }

    Ok(())
}

/// Ensures the FTS5 index on `symbols` exists at the specified database file path.
pub fn ensure_fts_index_path(path: &Path) -> Result<(), DbError> {
    if !path.exists() {
        return Err(DbError::NotFound(path.display().to_string()));
    }
    let conn = open_read_write(path)?;
    ensure_fts_index(&conn).map_err(DbError::PragmaFailed)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_open_read_write_and_read_only() {
        let dir = crate::safe_tempdir();
        let db_path = dir.path().join("test.db");
        let conn_rw = open_read_write(&db_path).unwrap();
        conn_rw
            .execute("CREATE TABLE test (id INTEGER PRIMARY KEY, name TEXT);", [])
            .unwrap();
        conn_rw
            .execute("INSERT INTO test (name) VALUES ('alpha');", [])
            .unwrap();
        drop(conn_rw);

        let conn_ro = open_read_only(&db_path).unwrap();
        let name: String = conn_ro
            .query_row("SELECT name FROM test WHERE id = 1", [], |r| r.get(0))
            .unwrap();
        assert_eq!(name, "alpha");

        // Verifying query_only prevents writes
        let write_res = conn_ro.execute("INSERT INTO test (name) VALUES ('beta');", []);
        assert!(write_res.is_err());
    }

    #[test]
    fn test_fts5_support() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute("CREATE VIRTUAL TABLE test_fts USING fts5(content);", [])
            .unwrap();
        conn.execute(
            "INSERT INTO test_fts (content) VALUES ('hello world token search');",
            [],
        )
        .unwrap();
        let count: i64 = conn
            .query_row(
                "SELECT count(*) FROM test_fts WHERE test_fts MATCH 'token'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn test_ensure_fts_index_lifecycle() {
        let dir = crate::safe_tempdir();
        let db_path = dir.path().join("fts_lifecycle.db");
        let conn = open_read_write(&db_path).unwrap();

        // Create mock symbols table
        conn.execute_batch(
            "CREATE TABLE symbols (
                symbol_id TEXT PRIMARY KEY,
                name TEXT,
                signature TEXT,
                doc_comment TEXT
            );
            INSERT INTO symbols VALUES ('1', 'PaymentGateway', 'pub trait PaymentGateway', 'Core payment provider interface');
            INSERT INTO symbols VALUES ('2', 'StripeClient', 'pub struct StripeClient', 'Handles HTTP requests to stripe API');",
        )
        .unwrap();

        // Ensure FTS index initializes and rebuilds existing rows
        ensure_fts_index(&conn).unwrap();

        let count: i64 = conn
            .query_row(
                "SELECT count(*) FROM symbols_fts WHERE symbols_fts MATCH 'payment'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);

        // Test trigger on insert
        conn.execute(
            "INSERT INTO symbols VALUES ('3', 'RefundHandler', 'pub fn handle_refund()', 'Processes transaction refunds');",
            [],
        )
        .unwrap();

        let count: i64 = conn
            .query_row(
                "SELECT count(*) FROM symbols_fts WHERE symbols_fts MATCH 'refund'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);

        // Test trigger on delete
        conn.execute("DELETE FROM symbols WHERE symbol_id = '3';", [])
            .unwrap();
        let count: i64 = conn
            .query_row(
                "SELECT count(*) FROM symbols_fts WHERE symbols_fts MATCH 'refund'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 0);
    }
}
