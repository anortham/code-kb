use std::process::Command;

use insta_cmd::get_cargo_bin;

fn render_with_fuel(fuel: &str) -> std::process::Output {
    Command::new(get_cargo_bin("minijinja-cli"))
        .env("MINIJINJA_FUEL", "1")
        .args(["--fuel", fuel, "--no-newline", "-t", r#"{% for i in range(10) %}{{ i }}{% endfor %}"#, ""])
        .output()
        .unwrap()
}

fn assert_rendered_with_fuel(fuel: &str) {
    let output = render_with_fuel(fuel);
    assert!(
        output.status.success(),
        "explicit --fuel {fuel} must override an inherited limit; stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stdout, b"0123456789");
}

#[test]
fn explicit_zero_cli_fuel_overrides_environment() {
    assert_rendered_with_fuel("0");
}

#[test]
fn positive_cli_fuel_overrides_environment() {
    assert_rendered_with_fuel("100");
}
