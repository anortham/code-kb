use code_kb_core::{
    Workspace, delete_file, ensure_fts_index_path, find_julie_extract_binary, open_read_only,
    safe_tempdir, scan_workspace, update_file,
};
use std::fs;
use std::path::Path;

const SIDECAR_FILE: &str = "src/sidecar.ts";

fn scanned_repo(files: &[(&str, &str)]) -> (tempfile::TempDir, std::path::PathBuf) {
    find_julie_extract_binary().expect("julie-extract binary must be present for tests");
    let temp_dir = safe_tempdir();
    let root = temp_dir.path().to_path_buf();
    for (path, content) in files {
        let full = root.join(path);
        fs::create_dir_all(full.parent().unwrap()).unwrap();
        fs::write(full, content).unwrap();
    }
    let db_path = root.join(".code-kb").join("artifact.db");
    scan_workspace(&Workspace::new(root), &db_path, true).expect("scan failed");
    (temp_dir, db_path)
}

fn trigram_names(db_path: &Path, term: &str) -> Vec<String> {
    let conn = open_read_only(db_path).unwrap();
    let mut stmt = conn
        .prepare("SELECT name FROM symbol_names_tri WHERE symbol_names_tri MATCH ?1 ORDER BY name")
        .unwrap();
    stmt.query_map([format!("\"{term}\"")], |r| r.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

#[test]
fn extractor_writes_keep_the_trigram_table_in_sync() {
    let (dir, db_path) = scanned_repo(&[(
        SIDECAR_FILE,
        "function parseSha256Sidecar(text: string): string {\n  return text.trim();\n}\n",
    )]);
    let workspace = Workspace::new(dir.path().to_path_buf());

    ensure_fts_index_path(&db_path).unwrap();
    assert_eq!(
        trigram_names(&db_path, "sha256"),
        vec!["parseSha256Sidecar"]
    );
    assert!(trigram_names(&db_path, "text").is_empty());

    fs::write(
        dir.path().join(SIDECAR_FILE),
        "function parseChecksumSidecar(text: string): string {\n  return text.trim();\n}\n",
    )
    .unwrap();
    update_file(&workspace, &db_path, SIDECAR_FILE).unwrap();
    assert!(trigram_names(&db_path, "sha256").is_empty());
    assert_eq!(
        trigram_names(&db_path, "checksum"),
        vec!["parseChecksumSidecar"]
    );

    fs::remove_file(dir.path().join(SIDECAR_FILE)).unwrap();
    delete_file(&workspace, &db_path, SIDECAR_FILE).unwrap();
    assert!(trigram_names(&db_path, "checksum").is_empty());
}
