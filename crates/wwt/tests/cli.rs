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
