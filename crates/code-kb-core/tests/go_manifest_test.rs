use code_kb_core::{
    Workspace, find_structural_facts_scoped, get_file, load_file_symbols, open_read_only,
    safe_tempdir, scan_workspace,
};
use std::fs;

#[test]
fn go_module_and_checksum_rows_are_queryable() {
    let dir = safe_tempdir();
    fs::write(
        dir.path().join("go.mod"),
        "module example.com/app\n\ngo 1.23\n\nrequire example.com/dep v1.2.3\n",
    )
    .unwrap();
    fs::write(
        dir.path().join("go.sum"),
        "example.com/dep v1.2.3 h1:AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=\n",
    )
    .unwrap();

    let db_path = dir.path().join(".code-kb/artifact.db");
    scan_workspace(&Workspace::new(dir.path().to_path_buf()), &db_path, false).unwrap();
    let conn = open_read_only(&db_path).unwrap();

    assert_eq!(
        get_file(&conn, "go.mod").unwrap().unwrap().language,
        "gomod"
    );
    assert_eq!(
        get_file(&conn, "go.sum").unwrap().unwrap().language,
        "gosum"
    );
    assert!(
        load_file_symbols(&conn, "go.mod")
            .unwrap()
            .iter()
            .any(|symbol| { symbol.name == "example.com/app" && symbol.kind == "module" })
    );
    assert_eq!(
        find_structural_facts_scoped(&conn, "manifest.dependency.v1", Some("go.mod"), 10)
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        find_structural_facts_scoped(&conn, "gosum.checksum.v1", Some("go.sum"), 10)
            .unwrap()
            .len(),
        1
    );
}
