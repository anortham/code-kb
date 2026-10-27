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
    let normalized = help
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase();
    let autoescape_help = normalized
        .split("--autoescape")
        .nth(1)
        .and_then(|section| section.split("[env var: minijinja_autoescape]").next())
        .unwrap_or("");

    for suffix in [".j2", ".jinja", ".jinja2"] {
        assert!(autoescape_help.contains(suffix), "{help}");
    }
    assert!(
        [
            "ignoring case",
            "case-insensitive",
            "case insensitive",
            "regardless of case",
            "without regard to case",
        ]
        .iter()
        .any(|phrase| autoescape_help.contains(phrase)),
        "{help}"
    );
}
