use std::fs;
use std::process::Command;
use std::thread;
use std::time::Duration;

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
        r#"terminal = ["printf", "%s\\n"]"#,
    )
    .expect("write launcher config");

    let output = Command::new(env!("CARGO_BIN_EXE_wwt"))
        .args(["--launch", "--new", "https://example.com"])
        .env("XDG_CONFIG_HOME", config_home.path())
        .output()
        .expect("run desktop launcher mode");

    assert!(output.status.success(), "status was {}", output.status);
    assert_eq!(
        String::from_utf8(output.stdout).expect("launcher output is UTF-8"),
        format!(
            "{}\n--new\nhttps://example.com\n",
            env!("CARGO_BIN_EXE_wwt")
        )
    );
    assert!(output.stderr.is_empty());
}

#[test]
fn launcher_can_open_wwt_without_a_url() {
    let config_home = tempfile::tempdir().expect("create temporary config home");
    let config_dir = config_home.path().join("wwt");
    let pid_file = config_home.path().join("terminal.pid");
    let arguments_file = config_home.path().join("terminal.arguments");
    fs::create_dir(&config_dir).expect("create wwt config directory");
    fs::write(
        config_dir.join("config.toml"),
        r#"terminal = ["sh", "-c", "printf '%s\\n' \"$$\" > \"$WWT_TEST_PID\"; printf '%s\\n' \"$@\" > \"$WWT_TEST_ARGUMENTS\"; exec sleep 30", "sh"]"#,
    )
    .expect("write launcher config");

    let status = Command::new(env!("CARGO_BIN_EXE_wwt"))
        .arg("--launch")
        .env("XDG_CONFIG_HOME", config_home.path())
        .env("WWT_TEST_PID", &pid_file)
        .env("WWT_TEST_ARGUMENTS", &arguments_file)
        .status()
        .expect("run desktop launcher mode without a URL");

    for _ in 0..100 {
        if pid_file.exists() && arguments_file.exists() {
            break;
        }
        thread::sleep(Duration::from_millis(10));
    }

    let pid = fs::read_to_string(&pid_file).unwrap_or_default();
    let pid = pid.trim();
    let child_was_running = !pid.is_empty()
        && Command::new("kill")
            .args(["-0", pid])
            .status()
            .is_ok_and(|status| status.success());
    let arguments = fs::read_to_string(&arguments_file).unwrap_or_default();
    if !pid.is_empty() {
        let _ = Command::new("kill").args(["-TERM", pid]).status();
    }

    assert!(status.success(), "status was {status}");
    assert!(child_was_running, "terminal child {pid:?} was not running");
    assert_eq!(arguments, format!("{}\n", env!("CARGO_BIN_EXE_wwt")));
}
