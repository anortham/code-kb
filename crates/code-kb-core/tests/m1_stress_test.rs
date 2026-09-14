#[allow(unused_imports)]
use code_kb_core::workspace::{
    Workspace, normalize_path, parse_file_uri, paths_equal, strip_prefix_lossy, to_forward_slash,
};
use std::path::{Path, PathBuf};

#[test]
fn stress_test_paths_equal_drive_casing() {
    #[cfg(windows)]
    {
        // Simple drive casing
        assert!(paths_equal(
            Path::new(r"C:\source\code-kb"),
            Path::new(r"c:\source\code-kb")
        ));
        assert!(paths_equal(
            Path::new(r"D:\my\project\file.rs"),
            Path::new(r"d:\my\project\file.rs")
        ));
        assert!(paths_equal(Path::new(r"Z:\Test"), Path::new(r"z:\Test")));
        // Mismatched drives must NEVER be equal
        assert!(!paths_equal(
            Path::new(r"C:\source\code-kb"),
            Path::new(r"D:\source\code-kb")
        ));
    }
}

#[test]
fn stress_test_paths_equal_slashes_and_trailing() {
    #[cfg(windows)]
    {
        // Forward vs backslashes
        assert!(paths_equal(
            Path::new("C:/source/code-kb/src/lib.rs"),
            Path::new(r"C:\source\code-kb\src\lib.rs")
        ));
        // Mixed slashes
        assert!(paths_equal(
            Path::new(r"C:\source/code-kb\src/lib.rs"),
            Path::new("C:/source\\code-kb/src\\lib.rs")
        ));
        // Trailing slashes
        assert!(paths_equal(
            Path::new(r"C:\source\code-kb\"),
            Path::new(r"C:\source\code-kb")
        ));
        assert!(paths_equal(
            Path::new("C:/source/code-kb/"),
            Path::new(r"C:\source\code-kb")
        ));
        assert!(paths_equal(
            Path::new(r"C:\source\code-kb\\\"),
            Path::new(r"C:\source\code-kb")
        ));
    }
}

#[test]
fn stress_test_paths_equal_verbatim_prefixes() {
    #[cfg(windows)]
    {
        // Standard verbatim disk
        assert!(paths_equal(
            Path::new(r"\\?\C:\source\code-kb"),
            Path::new(r"C:\source\code-kb")
        ));
        assert!(paths_equal(
            Path::new(r"\\?\c:\source\code-kb"),
            Path::new(r"C:\source\code-kb")
        ));
        assert!(paths_equal(
            Path::new(r"\\?\C:\source\code-kb"),
            Path::new(r"\\?\c:\source\code-kb")
        ));
        // Verbatim with forward slashes (normalize_path converts to backslashes)
        assert!(paths_equal(
            Path::new(r"\\?\C:/source/code-kb"),
            Path::new(r"C:\source\code-kb")
        ));
        // UNC verbatim
        assert!(paths_equal(
            Path::new(r"\\?\UNC\server\share\file.rs"),
            Path::new(r"\\server\share\file.rs")
        ));
        assert!(paths_equal(
            Path::new(r"\\?\UNC\SERVER\SHARE\FILE.RS"),
            Path::new(r"\\server\share\file.rs")
        ));
        assert!(paths_equal(
            Path::new(r"\\?\UNC\server\share\file.rs"),
            Path::new(r"\\?\UNC\SERVER\SHARE\FILE.RS")
        ));
    }
}

#[test]
fn stress_test_paths_equal_case_insensitivity_and_negatives() {
    #[cfg(windows)]
    {
        // Full path casing
        assert!(paths_equal(
            Path::new(r"C:\Foo\Bar\Baz.TXT"),
            Path::new(r"c:\foo\bar\baz.txt")
        ));
        // Different filenames
        assert!(!paths_equal(
            Path::new(r"C:\Foo\Bar\Baz.TXT"),
            Path::new(r"C:\Foo\Bar\Quux.TXT")
        ));
        // Different parent directories
        assert!(!paths_equal(
            Path::new(r"C:\Foo\Bar1\Baz.TXT"),
            Path::new(r"C:\Foo\Bar2\Baz.TXT")
        ));
        // Prefix vs extension mismatch
        assert!(!paths_equal(
            Path::new(r"C:\Foo\Bar"),
            Path::new(r"C:\Foo\Bar\Baz")
        ));
        // Prefix name mismatch
        assert!(!paths_equal(
            Path::new(r"C:\Foo\Bar"),
            Path::new(r"C:\Foo\BarExtra")
        ));
    }
}

#[test]
fn stress_test_paths_equal_relative_paths() {
    // Relative paths
    assert!(paths_equal(
        Path::new("src/lib.rs"),
        Path::new("src/lib.rs")
    ));
    #[cfg(windows)]
    {
        assert!(paths_equal(
            Path::new("src/lib.rs"),
            Path::new(r"src\lib.rs")
        ));
        assert!(paths_equal(
            Path::new("SRC/LIB.RS"),
            Path::new(r"src\lib.rs")
        ));
        assert!(paths_equal(
            Path::new("src/models/"),
            Path::new("src/models")
        ));
    }
}

#[test]
fn stress_test_parse_file_uri_all_variants() {
    #[cfg(windows)]
    {
        // Standard three-slash uppercase drive
        let p1 = parse_file_uri("file:///C:/source/code-kb/src/lib.rs").unwrap();
        assert_eq!(
            p1,
            normalize_path(Path::new("C:/source/code-kb/src/lib.rs"))
        );

        // Standard three-slash lowercase drive
        let p2 = parse_file_uri("file:///c:/source/code-kb/src/lib.rs").unwrap();
        assert_eq!(
            p2,
            normalize_path(Path::new("c:/source/code-kb/src/lib.rs"))
        );

        // Two-slash uppercase drive
        let p3 = parse_file_uri("file://C:/source/code-kb/src/lib.rs").unwrap();
        assert_eq!(
            p3,
            normalize_path(Path::new("C:/source/code-kb/src/lib.rs"))
        );

        // Two-slash lowercase drive
        let p4 = parse_file_uri("file://c:/source/code-kb/src/lib.rs").unwrap();
        assert_eq!(
            p4,
            normalize_path(Path::new("c:/source/code-kb/src/lib.rs"))
        );

        // Backslashes in three-slash URI
        let p7 = parse_file_uri(r"file:///C:\source\code-kb\src\lib.rs").unwrap();
        assert!(
            paths_equal(&p7, Path::new(r"C:\source\code-kb\src\lib.rs")),
            "Three-slash backslash URI failed: {:?}",
            p7
        );

        // Backslashes in two-slash URI
        let p8 = parse_file_uri(r"file://C:\source\code-kb\src\lib.rs").unwrap();
        assert!(
            paths_equal(&p8, Path::new(r"C:\source\code-kb\src\lib.rs")),
            "Two-slash backslash URI failed: {:?}",
            p8
        );

        // Pipe drive syntax observation
        let pipe_res1 = parse_file_uri("file:///C|/source/code-kb/src/lib.rs");
        let pipe_res2 = parse_file_uri("file://C|/source/code-kb/src/lib.rs");
        println!("pipe_res1: {:?}", pipe_res1);
        println!("pipe_res2: {:?}", pipe_res2);
    }
}

#[test]
fn stress_test_parse_file_uri_edge_cases() {
    #[cfg(windows)]
    {
        // Double percent-encoding: %2520 should decode to literal '%20'
        let p_double = parse_file_uri("file:///C:/projects/foo%2520bar.rs").unwrap();
        assert_eq!(
            p_double,
            normalize_path(Path::new(r"C:\projects\foo%20bar.rs")),
            "Double percent-encoding %2520 should result in literal %20"
        );

        // Invalid hex in percent encoding (e.g. %GG) should not panic and preserve text
        let p_bad_hex = parse_file_uri("file:///C:/projects/foo%GGbar.rs").unwrap();
        assert!(
            p_bad_hex.to_string_lossy().contains("foo"),
            "Invalid hex percent encoding should gracefully preserve text"
        );

        // Incomplete percent encoding at end of string (% at end)
        let p_trailing_pct = parse_file_uri("file:///C:/projects/foo%").unwrap();
        assert!(
            p_trailing_pct.to_string_lossy().contains("foo"),
            "Incomplete percent encoding should not panic"
        );

        // Bare file:// and file:///
        let p_empty1 = parse_file_uri("file://");
        assert!(p_empty1.is_some());
        let p_empty2 = parse_file_uri("file:///");
        assert!(p_empty2.is_some());

        // Windows drive root URI: file:///C:/ and file://C:/
        let p_root1 = parse_file_uri("file:///C:/").unwrap();
        let p_root2 = parse_file_uri("file://C:/").unwrap();
        let p_root3 = parse_file_uri("file:///c:/").unwrap();
        assert!(paths_equal(&p_root1, &p_root2));
        assert!(paths_equal(&p_root1, &p_root3));
        assert!(paths_equal(&p_root1, Path::new(r"C:\")));
    }
}

#[test]
fn stress_test_paths_equal_advanced_edge_cases() {
    #[cfg(windows)]
    {
        // UNC server and share case-insensitivity
        assert!(paths_equal(
            Path::new(r"\\Server\Share\Folder\File.rs"),
            Path::new(r"\\server\share\folder\file.rs")
        ));
        assert!(paths_equal(
            Path::new(r"\\?\UNC\Server\Share\Folder\File.rs"),
            Path::new(r"\\server\share\folder\file.rs")
        ));

        // Verbatim prefix with trailing slash
        assert!(paths_equal(
            Path::new(r"\\?\C:\source\code-kb\"),
            Path::new(r"C:\source\code-kb")
        ));
        assert!(paths_equal(
            Path::new(r"\\?\C:/source/code-kb/"),
            Path::new(r"C:\source\code-kb")
        ));

        // Root path variations
        assert!(paths_equal(Path::new(r"C:\"), Path::new(r"c:\")));
        assert!(paths_equal(Path::new(r"C:/"), Path::new(r"c:\")));
        assert!(paths_equal(Path::new(r"\\?\C:\"), Path::new(r"C:\")));

        // Note on relative paths with dot:
        // paths_equal compares components directly.
        // "./src/lib.rs" has Component::CurDir then "src", while "src/lib.rs" does not.
        println!(
            "paths_equal dot relative: {}",
            paths_equal(Path::new("./src/lib.rs"), Path::new("src/lib.rs"))
        );
    }
}

#[test]
fn stress_test_parse_file_uri_percent_encoding() {
    #[cfg(windows)]
    {
        // Space as %20
        let p1 = parse_file_uri("file:///C:/Program%20Files/My%20App/file.rs").unwrap();
        assert_eq!(
            p1,
            normalize_path(Path::new(r"C:\Program Files\My App\file.rs"))
        );

        // Two-slash with space
        let p2 = parse_file_uri("file://C:/Program%20Files/My%20App/file.rs").unwrap();
        assert_eq!(
            p2,
            normalize_path(Path::new(r"C:\Program Files\My App\file.rs"))
        );

        // Special characters percent-encoded
        // %5B = '[', %5D = ']'
        let p3 = parse_file_uri("file:///C:/projects/%5Bslug%5D/index.ts").unwrap();
        assert_eq!(
            p3,
            normalize_path(Path::new(r"C:\projects\[slug]\index.ts"))
        );

        // %40 = '@'
        let p4 = parse_file_uri("file:///C:/projects/%40org/pkg/index.ts").unwrap();
        assert_eq!(
            p4,
            normalize_path(Path::new(r"C:\projects\@org\pkg\index.ts"))
        );

        // Literal space (already decoded)
        let p5 = parse_file_uri("C:/Program Files/My App/file.rs").unwrap();
        assert_eq!(
            p5,
            normalize_path(Path::new(r"C:\Program Files\My App\file.rs"))
        );
    }
}

#[test]
fn stress_test_parse_file_uri_query_and_fragment() {
    #[cfg(windows)]
    {
        // Query parameters should be handled or ignored by URL parser
        if let Some(p) = parse_file_uri("file:///C:/source/lib.rs?v=1") {
            assert!(
                paths_equal(&p, Path::new(r"C:\source\lib.rs")),
                "Expected parsed path to equal C:\\source\\lib.rs, got: {:?}",
                p
            );
        }

        // Fragment should be handled or ignored by URL parser
        if let Some(p) = parse_file_uri("file:///C:/source/lib.rs#L10") {
            assert!(
                paths_equal(&p, Path::new(r"C:\source\lib.rs")),
                "Expected parsed path to equal C:\\source\\lib.rs, got: {:?}",
                p
            );
        }
    }
}

#[test]
fn stress_test_parse_file_uri_unc() {
    #[cfg(windows)]
    {
        let p = parse_file_uri("file://server/share/folder/file.rs");
        assert!(p.is_some(), "UNC file URI should be parseable");
        let p = p.unwrap();
        assert!(
            paths_equal(&p, Path::new(r"\\server\share\folder\file.rs")),
            "Expected UNC path, got: {:?}",
            p
        );
    }
}

#[test]
fn stress_test_workspace_new_variations() {
    let temp = code_kb_core::safe_tempdir();
    let temp_path = temp.path();

    // 1. Plain path
    let ws1 = Workspace::new(temp_path.to_path_buf());
    assert!(!ws1.root.to_string_lossy().starts_with(r"\\?\"));
    assert!(!ws1.canonical_root.to_string_lossy().starts_with(r"\\?\"));

    // 2. Verbatim prefix
    let verbatim = format!(r"\\?\{}", temp_path.display());
    let ws2 = Workspace::new(PathBuf::from(&verbatim));
    assert!(!ws2.root.to_string_lossy().starts_with(r"\\?\"));
    assert!(!ws2.canonical_root.to_string_lossy().starts_with(r"\\?\"));
    assert!(paths_equal(&ws1.canonical_root, &ws2.canonical_root));

    // 3. File URI
    let uri = format!("file:///{}", temp_path.to_string_lossy().replace('\\', "/"));
    let ws3 = Workspace::new(PathBuf::from(&uri));
    assert!(!ws3.root.to_string_lossy().starts_with("file://"));
    assert!(paths_equal(&ws1.canonical_root, &ws3.canonical_root));

    // 4. Two-slash File URI
    let uri_two = format!("file://{}", temp_path.to_string_lossy().replace('\\', "/"));
    let ws4 = Workspace::new(PathBuf::from(&uri_two));
    assert!(!ws4.root.to_string_lossy().starts_with("file://"));
    assert!(paths_equal(&ws1.canonical_root, &ws4.canonical_root));

    // 5. Trailing slash
    let trailing = format!("{}/", temp_path.display());
    let ws5 = Workspace::new(PathBuf::from(&trailing));
    assert!(paths_equal(&ws1.canonical_root, &ws5.canonical_root));
}

#[test]
fn stress_test_strip_prefix_lossy_matrix() {
    #[cfg(windows)]
    {
        let base = Path::new(r"C:\source\code-kb");
        let path = Path::new(r"c:\SOURCE\CODE-KB\src\commands\mod.rs");
        let rel = strip_prefix_lossy(path, base);
        assert_eq!(rel, Some(Path::new(r"src\commands\mod.rs")));

        // Mixed slashes in path
        let path_mixed = Path::new(r"c:/source/code-kb/src/commands/mod.rs");
        let rel_mixed = strip_prefix_lossy(path_mixed, base);
        assert!(rel_mixed.is_some());
        assert_eq!(to_forward_slash(rel_mixed.unwrap()), "src/commands/mod.rs");

        // Verbatim prefix in path
        let path_verbatim = Path::new(r"\\?\C:\source\code-kb\src\commands\mod.rs");
        let norm = normalize_path(path_verbatim);
        let rel_v = strip_prefix_lossy(&norm, base);
        assert_eq!(rel_v, Some(Path::new(r"src\commands\mod.rs")));

        // Negative: completely different directory
        let path_diff = Path::new(r"C:\other\project\src\lib.rs");
        assert_eq!(strip_prefix_lossy(path_diff, base), None);

        // Negative: same prefix string but different folder name
        let path_sibling = Path::new(r"C:\source\code-kb-extra\src\lib.rs");
        assert_eq!(strip_prefix_lossy(path_sibling, base), None);
    }
}
