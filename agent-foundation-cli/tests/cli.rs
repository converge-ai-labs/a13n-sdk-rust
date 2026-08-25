use std::process::{Command, Output};

fn run(argument: &str) -> Output {
    Command::new(env!("CARGO_BIN_EXE_agent-foundation"))
        .arg(argument)
        .output()
        .expect("agent-foundation should start")
}

fn normalized_stdout(output: &Output) -> String {
    String::from_utf8(output.stdout.clone())
        .expect("stdout should be UTF-8")
        .replace("\r\n", "\n")
}

#[test]
fn help_describes_the_cli() {
    let output = run("--help");

    assert!(output.status.success());
    let stdout = normalized_stdout(&output);
    assert!(stdout.contains("Command-line client for Agent Foundation Service"));
    assert!(stdout.contains("Usage: agent-foundation"));
}

#[test]
fn version_matches_the_package_version() {
    let output = run("--version");

    assert!(output.status.success());
    assert_eq!(
        normalized_stdout(&output),
        format!("agent-foundation {}\n", env!("CARGO_PKG_VERSION"))
    );
}
