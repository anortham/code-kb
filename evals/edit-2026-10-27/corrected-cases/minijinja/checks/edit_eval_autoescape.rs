use std::io::Write;
use std::process::Command;

use tempfile::Builder;

fn render(suffix: &str, autoescape: Option<&str>) -> String {
    let mut template = Builder::new().suffix(suffix).tempfile().unwrap();
    template.write_all(b"{{ value }}").unwrap();

    let mut command = Command::new(env!("CARGO_BIN_EXE_minijinja-cli"));
    command
        .arg("--no-newline")
        .arg("--define")
        .arg("value=<tag>");
    if let Some(mode) = autoescape {
        command.arg("--autoescape").arg(mode);
    }

    let output = command
        .arg(template.path())
        .output()
        .expect("minijinja-cli should start");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

#[test]
fn automatic_mode_uses_the_output_suffix_before_a_template_wrapper() {
    assert_eq!(
        render(".html.j2", None),
        "&lt;tag&gt;",
        "legacy .j2 autoescape should remain enabled",
    );
    assert_eq!(render(".HTML.J2", None), "&lt;tag&gt;");
    assert_eq!(render(".html", None), "&lt;tag&gt;");
    assert_eq!(render(".xml.JINJA2", None), "&lt;tag&gt;");
    assert_eq!(render(".json", None), "\"<tag>\"");
    assert_eq!(render(".yaml", None), "\"<tag>\"");
    assert_eq!(render(".JSON.JINJA", None), "\"<tag>\"");
    assert_eq!(render(".html.j2.jinja", None), "<tag>");
}

#[test]
fn explicit_modes_override_template_wrapper_detection() {
    assert_eq!(render(".html.jinja", Some("none")), "<tag>");
    assert_eq!(render(".json.j2", Some("html")), "&lt;tag&gt;");
    assert_eq!(render(".html.j2", Some("json")), "\"<tag>\"");
}

fn autoescape_help_matches_v2(help: &str) -> bool {
    let normalized = help
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase();
    let Some((_, autoescape_section)) = normalized.split_once("--autoescape") else {
        return false;
    };
    let Some((autoescape_help, _)) =
        autoescape_section.split_once("[env var: minijinja_autoescape]")
    else {
        return false;
    };
    let autoescape_help = autoescape_help.replace("ascii letter case", "case");

    [".j2", ".jinja", ".jinja2"]
        .iter()
        .all(|suffix| contains_suffix_token(&autoescape_help, suffix))
        && [
            "ignoring case",
            "ignores case",
            "ignore case",
            "case-insensitive",
            "case insensitive",
            "regardless of case",
            "without regard to case",
        ]
        .iter()
        .any(|phrase| autoescape_help.contains(phrase))
}

fn contains_suffix_token(text: &str, suffix: &str) -> bool {
    text.match_indices(suffix).any(|(start, matched)| {
        text.as_bytes()
            .get(start + matched.len())
            .map_or(true, |next| !next.is_ascii_alphanumeric())
    })
}

#[test]
fn long_help_documents_template_wrapper_suffixes() {
    let output = Command::new(env!("CARGO_BIN_EXE_minijinja-cli"))
        .arg("--long-help")
        .output()
        .expect("minijinja-cli should start");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let help = String::from_utf8_lossy(&output.stdout);
    assert!(autoescape_help_matches_v2(&help), "{help}");

    for accepted in [
        "--autoescape .j2, .jinja, and .jinja2 suffixes are ASCII case-insensitive. [env var: minijinja_autoescape]",
        "--autoescape .j2, .jinja, and .jinja2 suffixes are matched without regard to ASCII letter case. [env var: minijinja_autoescape]",
        "--autoescape .j2, .jinja, and .jinja2 suffix matching ignores ASCII letter case. [env var: minijinja_autoescape]",
    ] {
        assert!(autoescape_help_matches_v2(accepted), "{accepted}");
    }

    for rejected in [
        "--autoescape .j2, .jinja, and .jinja2 suffixes are documented. [env var: minijinja_autoescape]",
        "--autoescape .jinja and .jinja2 suffixes are case-insensitive. [env var: minijinja_autoescape]",
        "--autoescape .j2 and .jinja2 suffixes are case-insensitive. [env var: minijinja_autoescape]",
        "--autoescape .j2 and .jinja suffixes are case-insensitive. [env var: minijinja_autoescape]",
        "--autoescape .j2, .jinja, and .jinja2 suffix behavior mentions letter case. [env var: minijinja_autoescape]",
        "--autoescape .j2, .jinja, and .jinja2 suffix matching is case-sensitive. [env var: minijinja_autoescape]",
        "--autoescape .j2, .jinja, and .jinja2 suffixes. [env var: minijinja_autoescape] suffix matching is case-insensitive.",
    ] {
        assert!(!autoescape_help_matches_v2(rejected), "{rejected}");
    }
}
