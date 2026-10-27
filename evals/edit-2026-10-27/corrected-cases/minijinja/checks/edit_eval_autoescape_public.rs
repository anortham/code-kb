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
fn automatic_autoescape_detects_wrappers_and_output_extensions() {
    for (suffix, expected) in [
        (".HTML.J2", "&lt;tag&gt;"),
        (".xml.JINJA", "&lt;tag&gt;"),
        (".json.JINJA2", "\"<tag>\""),
        (".html.j2.jinja", "<tag>"),
    ] {
        assert_eq!(render(suffix, None), expected, "{suffix}");
    }
}

#[test]
fn explicit_autoescape_mode_overrides_wrapper_detection() {
    assert_eq!(render(".html.jinja", Some("none")), "<tag>");
    assert_eq!(render(".json.j2", Some("html")), "&lt;tag&gt;");
}
