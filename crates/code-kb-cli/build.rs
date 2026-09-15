use std::path::PathBuf;
use std::process::Command;

fn main() {
    let manifest_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let repo_root = manifest_dir
        .parent()
        .and_then(|p| p.parent())
        .unwrap_or(&manifest_dir);
    let pins_file = repo_root.join("scripts").join("julie-pins.json");

    if pins_file.exists() {
        println!("cargo:rerun-if-changed=../../scripts/julie-pins.json");
    }
    if repo_root.join(".tools").join("julie-extract").exists() {
        println!("cargo:rerun-if-changed=../../.tools/julie-extract");
    }
    if repo_root.join(".tools").join("julie-extract.exe").exists() {
        println!("cargo:rerun-if-changed=../../.tools/julie-extract.exe");
    }
    println!("cargo:rerun-if-env-changed=CODE_KB_ALLOW_MISSING_JULIE_EXTRACT");
    println!("cargo:rerun-if-env-changed=JULIE_EXTRACT_BIN");

    // Check if bypass is requested
    if std::env::var("CODE_KB_ALLOW_MISSING_JULIE_EXTRACT").as_deref() == Ok("1") {
        println!(
            "cargo:warning=CODE_KB_ALLOW_MISSING_JULIE_EXTRACT=1 set; bypassing julie-extract build verification"
        );
        return;
    }

    // Read pinned version from scripts/julie-pins.json; if pins file is absent (e.g. packaged on crates.io), skip verification
    let pinned_version = if pins_file.exists() {
        let content =
            std::fs::read_to_string(&pins_file).expect("Failed to read scripts/julie-pins.json");
        parse_version(&content).unwrap_or_else(|| "2.42.3".to_string())
    } else {
        println!(
            "cargo:warning=scripts/julie-pins.json not found; bypassing julie-extract build verification"
        );
        return;
    };

    let exe_name = if cfg!(windows) {
        "julie-extract.exe"
    } else {
        "julie-extract"
    };

    // Locate candidate binaries
    let mut candidates = Vec::new();

    if let Ok(env_bin) = std::env::var("JULIE_EXTRACT_BIN") {
        candidates.push(PathBuf::from(env_bin));
    }

    candidates.push(repo_root.join(".tools").join(exe_name));

    // Check sibling julie-extractors checkout in development
    candidates.push(
        repo_root
            .parent()
            .unwrap_or(repo_root)
            .join("julie-extractors")
            .join("target")
            .join("release")
            .join(exe_name),
    );
    candidates.push(
        repo_root
            .parent()
            .unwrap_or(repo_root)
            .join("julie-extractors")
            .join("target")
            .join("debug")
            .join(exe_name),
    );

    if let Ok(path) = std::env::var("PATH") {
        for dir in std::env::split_paths(&path) {
            let p = dir.join(exe_name);
            if p.exists() {
                candidates.push(p);
                break;
            }
        }
    }

    let found_binary = candidates.into_iter().find(|p| p.exists());

    let bin_path = match found_binary {
        Some(b) => b,
        None => {
            eprintln!("\n===================================================================");
            eprintln!("error: Pinned julie-extract v{pinned_version} is not restored.");
            eprintln!("code-kb requires julie-extract v{pinned_version} for AST indexing.");
            eprintln!(
                "Run `scripts/restore-julie-extract.sh` (or `scripts/restore-julie-extract.ps1`)"
            );
            eprintln!("or place the binary in `.tools/julie-extract`.");
            eprintln!(
                "To build anyway and defer the check, set CODE_KB_ALLOW_MISSING_JULIE_EXTRACT=1."
            );
            eprintln!("===================================================================\n");
            std::process::exit(1);
        }
    };

    // Run --version on found binary
    let output = match Command::new(&bin_path).arg("--version").output() {
        Ok(out) => out,
        Err(e) => {
            eprintln!("\n===================================================================");
            eprintln!(
                "error: Could not execute `{}` to verify version: {e}",
                bin_path.display()
            );
            eprintln!("Re-run `scripts/restore-julie-extract.sh` to reinstall.");
            eprintln!("===================================================================\n");
            std::process::exit(1);
        }
    };

    if !output.status.success() {
        eprintln!("\n===================================================================");
        eprintln!(
            "error: `{}` --version exited with error code",
            bin_path.display()
        );
        eprintln!("===================================================================\n");
        std::process::exit(1);
    }

    let version_output = String::from_utf8_lossy(&output.stdout);
    let actual_version =
        extract_semver(&version_output).unwrap_or_else(|| version_output.trim().to_string());

    if actual_version != pinned_version {
        eprintln!("\n===================================================================");
        eprintln!(
            "error: Found julie-extract is v{actual_version} but code-kb pins v{pinned_version}."
        );
        eprintln!("Location: {}", bin_path.display());
        eprintln!("A stale or mismatched extractor can break AST indexing and schema contracts.");
        eprintln!("Run `scripts/restore-julie-extract.sh` to update `.tools/julie-extract`.");
        eprintln!(
            "To build anyway and bypass this check, set CODE_KB_ALLOW_MISSING_JULIE_EXTRACT=1."
        );
        eprintln!("===================================================================\n");
        std::process::exit(1);
    }
}

fn parse_version(content: &str) -> Option<String> {
    for line in content.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("\"version\":") {
            let v = rest
                .trim()
                .trim_matches(|c| c == '"' || c == ',' || c == ' ');
            return Some(v.to_string());
        }
    }
    None
}

fn extract_semver(s: &str) -> Option<String> {
    for token in s.split_whitespace() {
        let clean = token.trim_start_matches('v');
        let parts: Vec<&str> = clean.split('.').collect();
        if parts.len() == 3 && parts.iter().all(|p| p.chars().all(|c| c.is_ascii_digit())) {
            return Some(clean.to_string());
        }
    }
    None
}
