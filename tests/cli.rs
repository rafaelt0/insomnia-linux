use std::fs::OpenOptions;
use std::os::fd::OwnedFd;
use std::os::unix::net::UnixStream;
use std::process::{Command, Stdio};

#[test]
fn help_succeeds_when_stdout_reader_is_closed() {
    let (reader, writer) = UnixStream::pair().unwrap();
    drop(reader);
    let output = Command::new(env!("CARGO_BIN_EXE_insomnia-awake"))
        .arg("--help")
        .stdout(Stdio::from(OwnedFd::from(writer)))
        .output()
        .unwrap();

    assert!(output.status.success(), "status: {}, stderr: {}", output.status, String::from_utf8_lossy(&output.stderr));
    assert!(output.stderr.is_empty(), "{}", String::from_utf8_lossy(&output.stderr));
}

#[test]
fn help_reports_stdout_write_failures() {
    let output = Command::new(env!("CARGO_BIN_EXE_insomnia-awake"))
        .arg("--help")
        .stdout(OpenOptions::new().write(true).open("/dev/full").unwrap())
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert_eq!(output.status.code(), Some(1), "stderr: {stderr}");
    assert!(stderr.starts_with("insomnia-awake: "), "{stderr}");
    assert!(!stderr.contains("panicked"), "{stderr}");
}
