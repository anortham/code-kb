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
    #[error("Database error: {0}")]
    Sqlite(#[from] rusqlite::Error),
}

/// Opens a read-only SQLite connection configured for low-overhead WAL reads.
pub fn open_read_only(path: &Path) -> Result<Connection, DbError> {
    if !path.exists() {
        return Err(DbError::NotFound(path.display().to_string()));
    }

    let flags = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX;
    let conn = Connection::open_with_flags(path, flags)
        .map_err(|e| DbError::OpenFailed(path.display().to_string(), e))?;

    #[cfg(windows)]
    conn.execute_batch(
        "PRAGMA busy_timeout = 5000;
         PRAGMA query_only = ON;
         PRAGMA cache_size = -4000;
         PRAGMA mmap_size = 0;",
    )
    .map_err(DbError::PragmaFailed)?;

    #[cfg(not(windows))]
    conn.execute_batch(
        "PRAGMA busy_timeout = 5000;
         PRAGMA query_only = ON;
         PRAGMA cache_size = -4000;
         PRAGMA mmap_size = 268435456;",
    )
    .map_err(DbError::PragmaFailed)?;

    Ok(conn)
}

/// Safely flushes all committed transactions from the WAL file into the main database file
/// and truncates the WAL to zero bytes. Returns an error if the database is busy and unable to truncate.
pub fn checkpoint_truncate(conn: &Connection) -> Result<(), DbError> {
    let busy: i32 = conn
        .query_row("PRAGMA wal_checkpoint(TRUNCATE);", [], |r| r.get(0))
        .map_err(DbError::PragmaFailed)?;
    if busy != 0 {
        return Err(DbError::PragmaFailed(rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_BUSY),
            Some("wal_checkpoint(TRUNCATE) failed: database busy".to_string()),
        )));
    }
    Ok(())
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

/// SQL predicate that is true for a symbols row that is a local variable or a parameter:
/// a `variable` declared inside a function, method, or constructor, directly or through
/// enclosing variables such as closures.
pub fn local_variable_predicate(alias: &str) -> String {
    format!(
        "({alias}.kind = 'variable' AND EXISTS (
            WITH RECURSIVE ancestor(symbol_id, kind, parent_symbol_id) AS (
                SELECT p.symbol_id, p.kind, p.parent_symbol_id FROM symbols p
                WHERE p.symbol_id = {alias}.parent_symbol_id
                UNION
                SELECT p.symbol_id, p.kind, p.parent_symbol_id FROM symbols p
                JOIN ancestor a ON p.symbol_id = a.parent_symbol_id
                WHERE a.kind = 'variable'
            )
            SELECT 1 FROM ancestor WHERE kind IN ('function', 'method', 'constructor')))"
    )
}

/// Ensures the `symbols_fts` FTS5 virtual table and synchronization triggers exist in the SQLite
/// database, and that it holds every symbol except locals and parameters. An index whose row
/// count does not match that rule is repopulated once.
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

    let is_local = local_variable_predicate("s");
    let new_is_local = local_variable_predicate("new");
    let already_indexed = "EXISTS (SELECT 1 FROM symbols_fts_docsize d WHERE d.id = old.rowid)";

    conn.execute_batch(&format!(
        "CREATE VIRTUAL TABLE IF NOT EXISTS symbols_fts USING fts5(
            name,
            signature,
            doc_comment,
            content='symbols',
            content_rowid='rowid',
            tokenize='porter unicode61'
        );

        DROP TRIGGER IF EXISTS symbols_ai;
        DROP TRIGGER IF EXISTS symbols_ad;
        DROP TRIGGER IF EXISTS symbols_au;

        CREATE TRIGGER symbols_ai AFTER INSERT ON symbols BEGIN
            INSERT INTO symbols_fts(rowid, name, signature, doc_comment)
            SELECT new.rowid, new.name, new.signature, new.doc_comment
            WHERE NOT {new_is_local};
        END;

        CREATE TRIGGER symbols_ad AFTER DELETE ON symbols BEGIN
            INSERT INTO symbols_fts(symbols_fts, rowid, name, signature, doc_comment)
            SELECT 'delete', old.rowid, old.name, old.signature, old.doc_comment
            WHERE {already_indexed};
        END;

        CREATE TRIGGER symbols_au AFTER UPDATE ON symbols BEGIN
            INSERT INTO symbols_fts(symbols_fts, rowid, name, signature, doc_comment)
            SELECT 'delete', old.rowid, old.name, old.signature, old.doc_comment
            WHERE {already_indexed};
            INSERT INTO symbols_fts(rowid, name, signature, doc_comment)
            SELECT new.rowid, new.name, new.signature, new.doc_comment
            WHERE NOT {new_is_local};
        END;"
    ))?;

    let indexable_count: i64 = conn
        .query_row(
            &format!("SELECT count(*) FROM symbols s WHERE NOT {is_local}"),
            [],
            |r| r.get(0),
        )
        .unwrap_or(0);
    let indexed_count: i64 = conn
        .query_row("SELECT count(*) FROM symbols_fts_docsize", [], |r| r.get(0))
        .unwrap_or(0);

    if indexed_count != indexable_count {
        conn.execute(
            "INSERT INTO symbols_fts(symbols_fts) VALUES('delete-all')",
            [],
        )?;
        conn.execute(
            &format!(
                "INSERT INTO symbols_fts(rowid, name, signature, doc_comment)
                 SELECT s.rowid, s.name, s.signature, s.doc_comment
                 FROM symbols s WHERE NOT {is_local}"
            ),
            [],
        )?;
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

/// Retargets the `root_path` key in `artifact_metadata` to a new workspace canonical root.
/// This is essential when cloning or copying an artifact database (e.g. into a git worktree),
/// ensuring `julie-extract update`, `delete`, and `scan` recognize the new root without root mismatch errors.
pub fn retarget_artifact_root(db_path: &Path, new_root: &Path) -> Result<(), DbError> {
    if !db_path.exists() {
        return Err(DbError::NotFound(db_path.display().to_string()));
    }
    let conn = open_read_write(db_path)?;
    conn.execute(
        "CREATE TABLE IF NOT EXISTS artifact_metadata (key TEXT PRIMARY KEY, value TEXT)",
        [],
    )?;

    let existing_root: Option<String> = conn
        .query_row(
            "SELECT value FROM artifact_metadata WHERE key = 'root_path'",
            [],
            |r| r.get(0),
        )
        .ok();

    let root_str = if existing_root
        .as_deref()
        .is_some_and(|ex| ex.starts_with(r"\\?\") || ex.starts_with(r"\\.\"))
        || (existing_root.is_none() && cfg!(windows))
    {
        std::fs::canonicalize(new_root)
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|_| {
                let s = new_root.to_string_lossy();
                format!(r"\\?\{s}")
            })
    } else {
        new_root.to_string_lossy().to_string()
    };

    conn.execute(
        "INSERT INTO artifact_metadata (key, value) VALUES ('root_path', ?1)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        rusqlite::params![root_str],
    )?;

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
                kind TEXT,
                parent_symbol_id TEXT,
                signature TEXT,
                doc_comment TEXT
            );
            INSERT INTO symbols VALUES ('1', 'PaymentGateway', 'trait', NULL, 'pub trait PaymentGateway', 'Core payment provider interface');
            INSERT INTO symbols VALUES ('2', 'StripeClient', 'struct', NULL, 'pub struct StripeClient', 'Handles HTTP requests to stripe API');",
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
            "INSERT INTO symbols VALUES ('3', 'RefundHandler', 'function', NULL, 'pub fn handle_refund()', 'Processes transaction refunds');",
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

    #[test]
    fn test_mmap_size_configuration() {
        let dir = crate::safe_tempdir();
        let db_path = dir.path().join("mmap_test.db");
        let conn_rw = open_read_write(&db_path).unwrap();
        conn_rw.execute("CREATE TABLE t (x INT);", []).unwrap();
        drop(conn_rw);

        let conn_ro = open_read_only(&db_path).unwrap();
        let mmap_size: i64 = conn_ro
            .query_row("PRAGMA mmap_size;", [], |r| r.get(0))
            .unwrap();
        #[cfg(windows)]
        assert_eq!(
            mmap_size, 0,
            "mmap_size must be 0 on Windows to prevent file locks"
        );
        #[cfg(not(windows))]
        assert_eq!(
            mmap_size, 268435456,
            "mmap_size should be 256MB on non-Windows"
        );
    }

    #[test]
    fn test_checkpoint_truncate() {
        let dir = crate::safe_tempdir();
        let db_path = dir.path().join("wal_checkpoint.db");
        let conn_rw = open_read_write(&db_path).unwrap();
        conn_rw
            .execute("CREATE TABLE items (id INTEGER PRIMARY KEY, val TEXT);", [])
            .unwrap();
        conn_rw
            .execute("INSERT INTO items (val) VALUES ('persisted_val');", [])
            .unwrap();
        checkpoint_truncate(&conn_rw).expect("checkpoint_truncate should succeed");
        drop(conn_rw);

        let conn_ro = open_read_only(&db_path).unwrap();
        let val: String = conn_ro
            .query_row("SELECT val FROM items WHERE id = 1", [], |r| r.get(0))
            .unwrap();
        assert_eq!(val, "persisted_val");
    }

    #[test]
    fn test_retarget_artifact_root() {
        let dir = crate::safe_tempdir();
        let db_path = dir.path().join("retarget.db");

        // 1. Missing db file -> NotFound error
        let missing_path = dir.path().join("missing.db");
        assert!(matches!(
            retarget_artifact_root(&missing_path, dir.path()),
            Err(DbError::NotFound(_))
        ));

        // 2. Db without existing artifact_metadata table -> creates table and sets root_path
        {
            let conn = open_read_write(&db_path).unwrap();
            conn.execute("CREATE TABLE other (x INT);", []).unwrap();
        }
        let fresh_root = dir.path().join("fresh_root");
        std::fs::create_dir_all(&fresh_root).unwrap();
        retarget_artifact_root(&db_path, &fresh_root).expect("Must create table and succeed");
        {
            let conn = open_read_only(&db_path).unwrap();
            let val: String = conn
                .query_row(
                    "SELECT value FROM artifact_metadata WHERE key = 'root_path'",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            assert!(
                crate::workspace::paths_equal(Path::new(&val), &fresh_root),
                "Paths must be equal: got {val}, expected {}",
                fresh_root.display()
            );
        }

        // 3. Db with existing artifact_metadata -> updates root_path
        {
            let conn = open_read_write(&db_path).unwrap();
            conn.execute(
                "UPDATE artifact_metadata SET value = '/old/root' WHERE key = 'root_path';",
                [],
            )
            .unwrap();
        }

        let new_root = dir.path().join("new_root");
        std::fs::create_dir_all(&new_root).unwrap();
        retarget_artifact_root(&db_path, &new_root).expect("retargeting must succeed");

        {
            let conn = open_read_only(&db_path).unwrap();
            let val: String = conn
                .query_row(
                    "SELECT value FROM artifact_metadata WHERE key = 'root_path'",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            assert!(
                crate::workspace::paths_equal(Path::new(&val), &new_root),
                "Paths must be equal: got {val}, expected {}",
                new_root.display()
            );
        }

        // 4. Verbatim prefix preservation on Windows when existing root started with \\?\
        #[cfg(windows)]
        {
            {
                let conn = open_read_write(&db_path).unwrap();
                conn.execute(
                    "UPDATE artifact_metadata SET value = '\\\\?\\C:\\old\\root' WHERE key = 'root_path';",
                    [],
                )
                .unwrap();
            }

            retarget_artifact_root(&db_path, &new_root).expect("retargeting verbatim must succeed");

            let conn = open_read_only(&db_path).unwrap();
            let val: String = conn
                .query_row(
                    "SELECT value FROM artifact_metadata WHERE key = 'root_path'",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            assert!(
                val.starts_with(r"\\?\"),
                "Must preserve \\\\?\\ prefix when existing root had it: got {val}"
            );
        }
    }
}
