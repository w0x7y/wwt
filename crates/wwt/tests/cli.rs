use std::fs;
use std::process::Command;

#[test]
fn help_is_a_successful_request_that_does_not_start_the_browser() {
    let output = Command::new(env!("CARGO_BIN_EXE_wwt"))
        .arg("--help")
        .output()
        .expect("run wwt --help");

    assert!(output.status.success(), "status was {}", output.status);
    assert_eq!(
        String::from_utf8(output.stdout).expect("help is UTF-8"),
        "Usage: wwt [OPTIONS] [URL OR SEARCH]\n\n\
         Options:\n  \
           --new          Start without restoring the saved session\n  \
         -h, --help     Print help\n  \
         -V, --version  Print version\n"
    );
    assert!(output.stderr.is_empty());
}

#[test]
fn version_is_a_successful_request_that_does_not_start_the_browser() {
    let output = Command::new(env!("CARGO_BIN_EXE_wwt"))
        .arg("--version")
        .output()
        .expect("run wwt --version");

    assert!(output.status.success(), "status was {}", output.status);
    assert_eq!(
        String::from_utf8(output.stdout).expect("version is UTF-8"),
        format!("wwt {}\n", env!("CARGO_PKG_VERSION"))
    );
    assert!(output.stderr.is_empty());
}

#[test]
fn launcher_hands_wwt_and_its_arguments_to_the_configured_terminal() {
    let config_home = tempfile::tempdir().expect("create temporary config home");
    let config_dir = config_home.path().join("wwt");
    fs::create_dir(&config_dir).expect("create wwt config directory");
    fs::write(
        config_dir.join("config.toml"),
        r#"terminal = ["/usr/bin/printf", "%s\\n"]"#,
    )
    .expect("write launcher config");

    let output = Command::new(env!("CARGO_BIN_EXE_wwt"))
        .args(["--launch", "https://example.com"])
        .env("XDG_CONFIG_HOME", config_home.path())
        .output()
        .expect("run desktop launcher mode");

    assert!(output.status.success(), "status was {}", output.status);
    assert_eq!(
        String::from_utf8(output.stdout).expect("launcher output is UTF-8"),
        format!("{}\nhttps://example.com\n", env!("CARGO_BIN_EXE_wwt"))
    );
    assert!(output.stderr.is_empty());
}

#[test]
fn launcher_can_open_wwt_without_a_url() {
    let config_home = tempfile::tempdir().expect("create temporary config home");
    let config_dir = config_home.path().join("wwt");
    fs::create_dir(&config_dir).expect("create wwt config directory");
    fs::write(
        config_dir.join("config.toml"),
        r#"terminal = ["/usr/bin/printf", "%s\\n"]"#,
    )
    .expect("write launcher config");

    let output = Command::new(env!("CARGO_BIN_EXE_wwt"))
        .arg("--launch")
        .env("XDG_CONFIG_HOME", config_home.path())
        .output()
        .expect("run desktop launcher mode without a URL");

    assert!(output.status.success(), "status was {}", output.status);
    assert_eq!(
        String::from_utf8(output.stdout).expect("launcher output is UTF-8"),
        format!("{}\n", env!("CARGO_BIN_EXE_wwt"))
    );
    assert!(output.stderr.is_empty());
}
