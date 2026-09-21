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
    #[error("FTS index migration failed: {0}")]
    FtsMigration(rusqlite::Error),
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

/// Identifies the rule the FTS content was built under. A stored marker that differs from this
/// value means the index predates the rule and must be repopulated once.
const FTS_RULE: &str = "exclude-locals-v1+names-trigram-v1";

fn stored_fts_rule(conn: &Connection) -> Option<String> {
    conn.query_row(
        "SELECT value FROM artifact_metadata WHERE key = 'fts_rule'",
        [],
        |r| r.get(0),
    )
    .ok()
}

fn fts_content_is_missing(conn: &Connection, table: &str) -> bool {
    let table_exists = conn
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1",
            [table],
            |_| Ok(true),
        )
        .unwrap_or(false);
    if !table_exists {
        return true;
    }
    let has_symbols = conn
        .query_row("SELECT 1 FROM symbols LIMIT 1", [], |_| Ok(true))
        .unwrap_or(false);
    if !has_symbols {
        return false;
    }
    !conn
        .query_row(
            &format!("SELECT 1 FROM {table}_docsize LIMIT 1"),
            [],
            |_| Ok(true),
        )
        .unwrap_or(false)
}

/// The `fts5vocab` view over `symbols_fts`, read for the global document frequency of a term.
pub(crate) const VOCAB_TABLE: &str = "symbols_fts_vocab";

/// Creates the vocabulary view when it is absent. Best effort: it stores nothing, the index
/// works without it, and a writer holding the schema lock must not fail the migration.
fn ensure_vocab_table(conn: &Connection) {
    let _ = conn.execute_batch(&format!(
        "CREATE VIRTUAL TABLE IF NOT EXISTS {VOCAB_TABLE} USING fts5vocab('symbols_fts', 'row');"
    ));
}

fn fts_index_is_ready(conn: &Connection) -> bool {
    stored_fts_rule(conn).as_deref() == Some(FTS_RULE)
        && !fts_content_is_missing(conn, "symbols_fts")
        && !fts_content_is_missing(conn, "symbol_names_tri")
}

/// Ensures the `symbols_fts` (words) and `symbol_names_tri` (name trigrams) FTS5 virtual tables
/// and their synchronization triggers exist in the SQLite database, and that both hold every
/// symbol except locals and parameters. An index built under an earlier rule, or one left
/// without content or without one of the tables, is repopulated once inside a single
/// transaction; the rule marker is written last so an interrupted migration reruns.
/// The migration takes the write lock up front, so a second process waits for the busy
/// timeout instead of failing at once, and re-checks readiness under the lock so it never
/// repeats a migration another process just finished.
/// julie may write a local before its enclosing function, so the insert trigger also drops any
/// same-file variable that became local when its parent arrived.
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

    if fts_index_is_ready(conn) {
        ensure_vocab_table(conn);
        return Ok(());
    }

    let tx = rusqlite::Transaction::new_unchecked(conn, rusqlite::TransactionBehavior::Immediate)?;
    if fts_index_is_ready(&tx) {
        return Ok(());
    }

    let is_local = local_variable_predicate("s");
    let new_is_local = local_variable_predicate("new");
    let child_is_local = local_variable_predicate("c");
    let already_indexed = "EXISTS (SELECT 1 FROM symbols_fts_docsize d WHERE d.id = old.rowid)";
    let name_already_indexed =
        "EXISTS (SELECT 1 FROM symbol_names_tri_docsize d WHERE d.id = old.rowid)";

    tx.execute_batch(&format!(
        "CREATE VIRTUAL TABLE IF NOT EXISTS symbols_fts USING fts5(
            name,
            signature,
            doc_comment,
            content='symbols',
            content_rowid='rowid',
            tokenize='porter unicode61'
        );

        CREATE VIRTUAL TABLE IF NOT EXISTS symbol_names_tri USING fts5(
            name,
            content='symbols',
            content_rowid='rowid',
            tokenize='trigram'
        );

        DROP TRIGGER IF EXISTS symbols_ai;
        DROP TRIGGER IF EXISTS symbols_ad;
        DROP TRIGGER IF EXISTS symbols_au;

        CREATE TRIGGER symbols_ai AFTER INSERT ON symbols BEGIN
            INSERT INTO symbols_fts(rowid, name, signature, doc_comment)
            SELECT new.rowid, new.name, new.signature, new.doc_comment
            WHERE NOT {new_is_local};
            INSERT INTO symbol_names_tri(rowid, name)
            SELECT new.rowid, new.name
            WHERE NOT {new_is_local};
            INSERT INTO symbols_fts(symbols_fts, rowid, name, signature, doc_comment)
            SELECT 'delete', c.rowid, c.name, c.signature, c.doc_comment
            FROM symbols c
            WHERE new.kind IN ('function', 'method', 'constructor')
              AND c.path = new.path
              AND c.kind = 'variable'
              AND {child_is_local}
              AND EXISTS (SELECT 1 FROM symbols_fts_docsize d WHERE d.id = c.rowid);
            INSERT INTO symbol_names_tri(symbol_names_tri, rowid, name)
            SELECT 'delete', c.rowid, c.name
            FROM symbols c
            WHERE new.kind IN ('function', 'method', 'constructor')
              AND c.path = new.path
              AND c.kind = 'variable'
              AND {child_is_local}
              AND EXISTS (SELECT 1 FROM symbol_names_tri_docsize d WHERE d.id = c.rowid);
        END;

        CREATE TRIGGER symbols_ad AFTER DELETE ON symbols BEGIN
            INSERT INTO symbols_fts(symbols_fts, rowid, name, signature, doc_comment)
            SELECT 'delete', old.rowid, old.name, old.signature, old.doc_comment
            WHERE {already_indexed};
            INSERT INTO symbol_names_tri(symbol_names_tri, rowid, name)
            SELECT 'delete', old.rowid, old.name
            WHERE {name_already_indexed};
        END;

        CREATE TRIGGER symbols_au AFTER UPDATE ON symbols BEGIN
            INSERT INTO symbols_fts(symbols_fts, rowid, name, signature, doc_comment)
            SELECT 'delete', old.rowid, old.name, old.signature, old.doc_comment
            WHERE {already_indexed};
            INSERT INTO symbol_names_tri(symbol_names_tri, rowid, name)
            SELECT 'delete', old.rowid, old.name
            WHERE {name_already_indexed};
            INSERT INTO symbols_fts(rowid, name, signature, doc_comment)
            SELECT new.rowid, new.name, new.signature, new.doc_comment
            WHERE NOT {new_is_local};
            INSERT INTO symbol_names_tri(rowid, name)
            SELECT new.rowid, new.name
            WHERE NOT {new_is_local};
        END;

        INSERT INTO symbols_fts(symbols_fts) VALUES('delete-all');
        INSERT INTO symbol_names_tri(symbol_names_tri) VALUES('delete-all');

        INSERT INTO symbols_fts(rowid, name, signature, doc_comment)
        SELECT s.rowid, s.name, s.signature, s.doc_comment
        FROM symbols s WHERE NOT {is_local};
        INSERT INTO symbol_names_tri(rowid, name)
        SELECT s.rowid, s.name
        FROM symbols s WHERE NOT {is_local};

        CREATE TABLE IF NOT EXISTS artifact_metadata (key TEXT PRIMARY KEY, value TEXT);"
    ))?;
    tx.execute(
        "INSERT INTO artifact_metadata (key, value) VALUES ('fts_rule', ?1)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        [FTS_RULE],
    )?;
    tx.commit()?;
    ensure_vocab_table(conn);

    Ok(())
}

/// Ensures the FTS5 index on `symbols` exists at the specified database file path. The
/// connection waits up to 60 s for another process's migration; a large index takes seconds.
pub fn ensure_fts_index_path(path: &Path) -> Result<(), DbError> {
    if !path.exists() {
        return Err(DbError::NotFound(path.display().to_string()));
    }
    let conn = open_read_write(path)?;
    conn.busy_timeout(std::time::Duration::from_secs(60))
        .map_err(DbError::PragmaFailed)?;
    ensure_fts_index(&conn).map_err(DbError::FtsMigration)?;
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

        conn.execute_batch(
            "CREATE TABLE symbols (
                symbol_id TEXT PRIMARY KEY,
                path TEXT,
                name TEXT,
                kind TEXT,
                parent_symbol_id TEXT,
                signature TEXT,
                doc_comment TEXT
            );
            INSERT INTO symbols VALUES ('1', 'src/pay.rs', 'PaymentGateway', 'trait', NULL, 'pub trait PaymentGateway', 'Core payment provider interface');
            INSERT INTO symbols VALUES ('2', 'src/pay.rs', 'StripeClient', 'struct', NULL, 'pub struct StripeClient', 'Handles HTTP requests to stripe API');",
        )
        .unwrap();

        ensure_fts_index(&conn).unwrap();

        let count: i64 = conn
            .query_row(
                "SELECT count(*) FROM symbols_fts WHERE symbols_fts MATCH 'payment'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);

        conn.execute(
            "INSERT INTO symbols VALUES ('3', 'src/pay.rs', 'RefundHandler', 'function', NULL, 'pub fn handle_refund()', 'Processes transaction refunds');",
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
    fn ensure_fts_index_drops_a_local_indexed_before_its_parent() {
        let dir = crate::safe_tempdir();
        let db_path = dir.path().join("fts_rule.db");
        let conn = open_read_write(&db_path).unwrap();
        conn.execute_batch(
            "CREATE TABLE symbols (
                symbol_id TEXT PRIMARY KEY,
                path TEXT,
                name TEXT,
                kind TEXT,
                parent_symbol_id TEXT,
                signature TEXT,
                doc_comment TEXT
            );
            INSERT INTO symbols VALUES ('1', 'src/db.rs', 'open_conn', 'function', NULL, 'fn open_conn()', '');",
        )
        .unwrap();

        ensure_fts_index(&conn).unwrap();

        let rule: String = conn
            .query_row(
                "SELECT value FROM artifact_metadata WHERE key = 'fts_rule'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(rule, FTS_RULE);

        conn.execute(
            "INSERT INTO symbols VALUES ('2', 'src/db.rs', 'drifted', 'variable', '3', 'let drifted = 1', '')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO symbols VALUES ('3', 'src/db.rs', 'later', 'function', NULL, 'fn later()', '')",
            [],
        )
        .unwrap();

        let count: i64 = conn
            .query_row(
                "SELECT count(*) FROM symbols_fts WHERE symbols_fts MATCH 'drifted'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 0);
        assert!(trigram_names(&conn, "drifted").is_empty());
        assert_eq!(trigram_names(&conn, "later"), vec!["later"]);
    }

    #[test]
    fn ensure_fts_index_repopulates_an_emptied_index() {
        let dir = crate::safe_tempdir();
        let db_path = dir.path().join("fts_empty.db");
        let conn = open_read_write(&db_path).unwrap();
        conn.execute_batch(
            "CREATE TABLE symbols (
                symbol_id TEXT PRIMARY KEY,
                path TEXT,
                name TEXT,
                kind TEXT,
                parent_symbol_id TEXT,
                signature TEXT,
                doc_comment TEXT
            );
            INSERT INTO symbols VALUES ('1', 'src/pay.rs', 'RefundHandler', 'function', NULL, 'fn handle_refund()', '');",
        )
        .unwrap();

        ensure_fts_index(&conn).unwrap();
        conn.execute(
            "INSERT INTO symbols_fts(symbols_fts) VALUES('delete-all')",
            [],
        )
        .unwrap();

        ensure_fts_index(&conn).unwrap();

        let count: i64 = conn
            .query_row(
                "SELECT count(*) FROM symbols_fts WHERE symbols_fts MATCH 'refund'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);
    }

    fn symbols_db(file: &str) -> (tempfile::TempDir, Connection) {
        let dir = crate::safe_tempdir();
        let conn = open_read_write(&dir.path().join(file)).unwrap();
        conn.execute_batch(
            "CREATE TABLE symbols (
                symbol_id TEXT PRIMARY KEY,
                path TEXT,
                name TEXT,
                kind TEXT,
                parent_symbol_id TEXT,
                signature TEXT,
                doc_comment TEXT
            );
            INSERT INTO symbols VALUES ('1', 'src/sidecar.ts', 'parseSha256Sidecar', 'function', NULL, 'function parseSha256Sidecar()', 'Reads the checksum sidecar');
            INSERT INTO symbols VALUES ('2', 'src/sidecar.ts', 'digestBuffer', 'variable', '1', 'const digestBuffer', '');",
        )
        .unwrap();
        (dir, conn)
    }

    fn trigram_names(conn: &Connection, term: &str) -> Vec<String> {
        let mut stmt = conn
            .prepare(
                "SELECT name FROM symbol_names_tri WHERE symbol_names_tri MATCH ?1 ORDER BY name",
            )
            .unwrap();
        stmt.query_map([format!("\"{term}\"")], |r| r.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    }

    fn word_names(conn: &Connection, term: &str) -> Vec<String> {
        let mut stmt = conn
            .prepare("SELECT name FROM symbols_fts WHERE symbols_fts MATCH ?1 ORDER BY name")
            .unwrap();
        stmt.query_map([term], |r| r.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    }

    fn docsize_rows(conn: &Connection, table: &str) -> i64 {
        conn.query_row(&format!("SELECT count(*) FROM {table}_docsize"), [], |r| {
            r.get(0)
        })
        .unwrap()
    }

    fn schema_version(conn: &Connection) -> i64 {
        conn.query_row("PRAGMA schema_version", [], |r| r.get(0))
            .unwrap()
    }

    #[test]
    fn fresh_index_populates_both_tables_without_locals() {
        let (_dir, conn) = symbols_db("fresh.db");

        ensure_fts_index(&conn).unwrap();

        assert_eq!(trigram_names(&conn, "sha256"), vec!["parseSha256Sidecar"]);
        assert_eq!(word_names(&conn, "sidecar"), vec!["parseSha256Sidecar"]);
        assert!(trigram_names(&conn, "digest").is_empty());
        assert!(word_names(&conn, "digestBuffer").is_empty());
        assert_eq!(stored_fts_rule(&conn).as_deref(), Some(FTS_RULE));
    }

    fn exclude_locals_v1_layout(conn: &Connection) {
        conn.execute_batch(
            "CREATE VIRTUAL TABLE symbols_fts USING fts5(
                name, signature, doc_comment,
                content='symbols', content_rowid='rowid', tokenize='porter unicode61'
            );
            INSERT INTO symbols_fts(rowid, name, signature, doc_comment)
            SELECT rowid, name, signature, doc_comment FROM symbols WHERE kind != 'variable';
            CREATE TABLE artifact_metadata (key TEXT PRIMARY KEY, value TEXT);
            INSERT INTO artifact_metadata VALUES ('fts_rule', 'exclude-locals-v1');",
        )
        .unwrap();
    }

    #[test]
    fn upgrade_from_exclude_locals_v1_adds_the_trigram_table() {
        let (_dir, conn) = symbols_db("upgrade.db");
        exclude_locals_v1_layout(&conn);

        ensure_fts_index(&conn).unwrap();

        assert_eq!(trigram_names(&conn, "sha256"), vec!["parseSha256Sidecar"]);
        assert_eq!(word_names(&conn, "sidecar"), vec!["parseSha256Sidecar"]);
        assert_eq!(stored_fts_rule(&conn).as_deref(), Some(FTS_RULE));
    }

    #[test]
    fn migration_waits_for_another_writer_and_does_not_repeat_its_work() {
        let (dir, a) = symbols_db("contended.db");
        exclude_locals_v1_layout(&a);
        let b = open_read_write(&dir.path().join("contended.db")).unwrap();
        b.busy_timeout(std::time::Duration::from_secs(5)).unwrap();
        let (locked_tx, locked_rx) = std::sync::mpsc::channel();

        let writer = std::thread::spawn(move || {
            a.execute_batch("BEGIN IMMEDIATE").unwrap();
            locked_tx.send(()).unwrap();
            std::thread::sleep(std::time::Duration::from_millis(300));
            a.execute_batch("COMMIT").unwrap();
            ensure_fts_index(&a).unwrap();
            schema_version(&a)
        });
        locked_rx.recv().unwrap();

        ensure_fts_index(&b).unwrap();

        assert_eq!(schema_version(&b), writer.join().unwrap());
        assert_eq!(trigram_names(&b, "sha256"), vec!["parseSha256Sidecar"]);
        assert_eq!(word_names(&b, "sidecar"), vec!["parseSha256Sidecar"]);
        assert_eq!(stored_fts_rule(&b).as_deref(), Some(FTS_RULE));
    }

    #[test]
    fn interrupted_migration_is_completed_once_and_then_left_alone() {
        let (_dir, conn) = symbols_db("interrupted.db");
        ensure_fts_index(&conn).unwrap();
        conn.execute("DELETE FROM artifact_metadata WHERE key = 'fts_rule'", [])
            .unwrap();
        let before_repair = schema_version(&conn);

        ensure_fts_index(&conn).unwrap();

        assert_eq!(stored_fts_rule(&conn).as_deref(), Some(FTS_RULE));
        assert_eq!(trigram_names(&conn, "sha256"), vec!["parseSha256Sidecar"]);
        assert_ne!(schema_version(&conn), before_repair);
        let settled = schema_version(&conn);
        let rows = (
            docsize_rows(&conn, "symbols_fts"),
            docsize_rows(&conn, "symbol_names_tri"),
        );

        ensure_fts_index(&conn).unwrap();

        assert_eq!(schema_version(&conn), settled);
        assert_eq!(
            (
                docsize_rows(&conn, "symbols_fts"),
                docsize_rows(&conn, "symbol_names_tri")
            ),
            rows
        );
    }

    #[test]
    fn missing_trigram_table_is_recreated_despite_the_marker() {
        let (_dir, conn) = symbols_db("dropped.db");
        ensure_fts_index(&conn).unwrap();
        conn.execute("DROP TABLE symbol_names_tri", []).unwrap();

        ensure_fts_index(&conn).unwrap();

        assert_eq!(trigram_names(&conn, "sha256"), vec!["parseSha256Sidecar"]);
        assert_eq!(docsize_rows(&conn, "symbol_names_tri"), 1);
    }

    #[test]
    fn triggers_keep_the_trigram_table_in_sync() {
        let (_dir, conn) = symbols_db("triggers.db");
        ensure_fts_index(&conn).unwrap();

        conn.execute(
            "INSERT INTO symbols VALUES ('3', 'src/verify.ts', 'verifyChecksum', 'function', NULL, 'function verifyChecksum()', '')",
            [],
        )
        .unwrap();
        assert_eq!(trigram_names(&conn, "checksum"), vec!["verifyChecksum"]);

        conn.execute(
            "UPDATE symbols SET name = 'verifyDigest' WHERE symbol_id = '3'",
            [],
        )
        .unwrap();
        assert!(trigram_names(&conn, "checksum").is_empty());
        assert_eq!(trigram_names(&conn, "digest"), vec!["verifyDigest"]);

        conn.execute("DELETE FROM symbols WHERE symbol_id = '3'", [])
            .unwrap();
        assert!(trigram_names(&conn, "digest").is_empty());
        assert_eq!(docsize_rows(&conn, "symbol_names_tri"), 1);
    }

    #[test]
    fn a_read_only_connection_queries_the_vocab_table_after_the_migration() {
        let (dir, conn) = symbols_db("vocab.db");

        ensure_fts_index(&conn).unwrap();

        drop(conn);
        let reader = open_read_only(&dir.path().join("vocab.db")).unwrap();
        let doc: i64 = reader
            .query_row(
                &format!("SELECT doc FROM {VOCAB_TABLE} WHERE term = 'sidecar'"),
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(doc, 1);
    }

    #[test]
    fn a_ready_index_gains_the_vocab_table_without_a_rebuild() {
        let (_dir, conn) = symbols_db("vocab-upgrade.db");
        ensure_fts_index(&conn).unwrap();
        conn.execute_batch(&format!(
            "DROP TABLE {VOCAB_TABLE};
             DROP TRIGGER symbols_ai;
             INSERT INTO symbols VALUES ('3', 'src/g.ts', 'ghostSymbol', 'function', NULL, '', '');"
        ))
        .unwrap();

        ensure_fts_index(&conn).unwrap();

        assert!(crate::queries::has_table(&conn, VOCAB_TABLE));
        assert!(word_names(&conn, "ghostSymbol").is_empty());
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
